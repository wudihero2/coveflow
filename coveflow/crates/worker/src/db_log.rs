use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use tokio::sync::mpsc;
use tracing::field::{Field, Visit};
use tracing::span;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;

use coveflow_queue::{RunLogChunk, ServiceLogChunk};

pub struct LogEvent {
    pub timestamp: DateTime<Utc>,
    pub level: tracing::Level,
    pub target: String,
    pub message: String,
    pub fields: serde_json::Value,
    pub run_id: Option<uuid::Uuid>,
    pub instance_id: String,
    pub service: String,
}

#[derive(Clone, Debug)]
struct RunIdExtension(uuid::Uuid);

#[derive(Clone, Debug)]
struct DbLogSkipExtension;

/// Well-known field name: when set to `true` on an event or span,
/// DbLogLayer will skip writing it to the database.
/// Usage: `tracing::info!(db_log_skip = true, "internal message");`
pub const DB_LOG_SKIP_FIELD: &str = "db_log_skip";

struct EventVisitor {
    message: String,
    fields: serde_json::Map<String, serde_json::Value>,
    skip: bool,
}

impl EventVisitor {
    fn new() -> Self {
        Self {
            message: String::new(),
            fields: serde_json::Map::new(),
            skip: false,
        }
    }
}

impl Visit for EventVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{:?}", value);
        } else {
            self.fields.insert(
                field.name().to_string(),
                serde_json::json!(format!("{:?}", value)),
            );
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        } else {
            self.fields
                .insert(field.name().to_string(), serde_json::json!(value));
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.fields
            .insert(field.name().to_string(), serde_json::json!(value));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.fields
            .insert(field.name().to_string(), serde_json::json!(value));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        if field.name() == DB_LOG_SKIP_FIELD {
            self.skip = value;
            return;
        }
        self.fields
            .insert(field.name().to_string(), serde_json::json!(value));
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        self.fields
            .insert(field.name().to_string(), serde_json::json!(value));
    }
}

struct SpanFieldVisitor {
    run_id: Option<String>,
    skip: bool,
}

impl SpanFieldVisitor {
    fn new() -> Self {
        Self {
            run_id: None,
            skip: false,
        }
    }
}

impl Visit for SpanFieldVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "run_id" {
            self.run_id = Some(format!("{:?}", value));
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "run_id" {
            self.run_id = Some(value.to_string());
        }
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        if field.name() == DB_LOG_SKIP_FIELD {
            self.skip = value;
        }
    }
}

pub struct DbLogLayer {
    tx: mpsc::Sender<LogEvent>,
    instance_id: String,
    service: String,
    dropped: Arc<AtomicU64>,
}

impl DbLogLayer {
    /// Number of log events dropped due to full channel.
    pub fn dropped_count(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

impl<S> Layer<S> for DbLogLayer
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &span::Attributes<'_>, id: &span::Id, ctx: Context<'_, S>) {
        let mut visitor = SpanFieldVisitor::new();
        attrs.record(&mut visitor);

        if let Some(span) = ctx.span(id) {
            if let Some(run_id_str) = visitor.run_id {
                if let Ok(run_id) = run_id_str.parse::<uuid::Uuid>() {
                    span.extensions_mut().insert(RunIdExtension(run_id));
                }
            }
            if visitor.skip {
                span.extensions_mut().insert(DbLogSkipExtension);
            }
        }
    }

    fn on_event(&self, event: &tracing::Event<'_>, ctx: Context<'_, S>) {
        let mut visitor = EventVisitor::new();
        event.record(&mut visitor);

        if visitor.skip {
            return;
        }

        // Walk span scope to find run_id (and check for span-level skip)
        let mut run_id: Option<uuid::Uuid> = None;
        if let Some(scope) = ctx.event_span(event) {
            // Check current span first
            if scope.extensions().get::<DbLogSkipExtension>().is_some() {
                return;
            }
            if let Some(ext) = scope.extensions().get::<RunIdExtension>() {
                run_id = Some(ext.0);
            }
            // Walk parents
            for span in scope.scope().skip(1) {
                if span.extensions().get::<DbLogSkipExtension>().is_some() {
                    return;
                }
                if run_id.is_none() {
                    if let Some(ext) = span.extensions().get::<RunIdExtension>() {
                        run_id = Some(ext.0);
                    }
                }
            }
        }

        let log_event = LogEvent {
            timestamp: Utc::now(),
            level: *event.metadata().level(),
            target: event.metadata().target().to_string(),
            message: visitor.message,
            fields: serde_json::Value::Object(visitor.fields),
            run_id,
            instance_id: self.instance_id.clone(),
            service: self.service.clone(),
        };

        if self.tx.try_send(log_event).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

struct LogFlusher {
    rx: mpsc::Receiver<LogEvent>,
    db: PgPool,
    /// Per-run sequence counter
    run_seqs: HashMap<uuid::Uuid, AtomicI32>,
    /// Service log sequence counter
    service_seq: AtomicI32,
}

fn level_to_i16(level: &tracing::Level) -> i16 {
    match *level {
        tracing::Level::TRACE => 1,
        tracing::Level::DEBUG => 2,
        tracing::Level::INFO => 3,
        tracing::Level::WARN => 4,
        tracing::Level::ERROR => 5,
    }
}

impl LogFlusher {
    fn new(rx: mpsc::Receiver<LogEvent>, db: PgPool) -> Self {
        Self {
            rx,
            db,
            run_seqs: HashMap::new(),
            service_seq: AtomicI32::new(0),
        }
    }

    async fn run(mut self) {
        let mut buffer: Vec<LogEvent> = Vec::with_capacity(128);
        let flush_interval = tokio::time::Duration::from_millis(500);

        loop {
            tokio::select! {
                maybe_event = self.rx.recv() => {
                    match maybe_event {
                        Some(event) => {
                            buffer.push(event);

                            // Drain more events without blocking (up to 100 total)
                            while buffer.len() < 100 {
                                match self.rx.try_recv() {
                                    Ok(e) => buffer.push(e),
                                    Err(_) => break,
                                }
                            }

                            if buffer.len() >= 100 {
                                self.flush(&mut buffer).await;
                            }
                        }
                        None => {
                            // Channel closed — drain remaining and exit
                            self.flush(&mut buffer).await;
                            return;
                        }
                    }
                }

                () = tokio::time::sleep(flush_interval) => {
                    if !buffer.is_empty() {
                        self.flush(&mut buffer).await;
                    }
                }
            }
        }
    }

    async fn flush(&mut self, buffer: &mut Vec<LogEvent>) {
        if buffer.is_empty() {
            return;
        }

        // Partition events: has run_id -> run_log, no run_id -> service_log
        let mut run_events: HashMap<uuid::Uuid, Vec<&LogEvent>> = HashMap::new();
        let mut service_events: Vec<&LogEvent> = Vec::new();

        for event in buffer.iter() {
            if let Some(run_id) = event.run_id {
                run_events.entry(run_id).or_default().push(event);
            } else {
                service_events.push(event);
            }
        }

        // Build and insert run log chunks
        if !run_events.is_empty() {
            let mut chunks: Vec<RunLogChunk> = Vec::new();

            for (run_id, events) in &run_events {
                let seq_counter = self
                    .run_seqs
                    .entry(*run_id)
                    .or_insert_with(|| AtomicI32::new(0));
                let seq = seq_counter.fetch_add(1, Ordering::Relaxed);

                let mut min_level: i16 = i16::MAX;
                let mut max_level: i16 = i16::MIN;
                let mut entries = Vec::new();

                for event in events {
                    let lvl = level_to_i16(&event.level);
                    min_level = min_level.min(lvl);
                    max_level = max_level.max(lvl);

                    let mut entry = serde_json::json!({
                        "ts": event.timestamp.to_rfc3339(),
                        "level": lvl,
                        "msg": event.message,
                        "target": event.target,
                    });

                    if let serde_json::Value::Object(ref fields) = event.fields {
                        if !fields.is_empty() {
                            entry["fields"] = event.fields.clone();
                        }
                    }

                    entries.push(entry);
                }

                chunks.push(RunLogChunk {
                    run_id: *run_id,
                    seq,
                    min_level,
                    max_level,
                    line_count: entries.len() as i16,
                    entries: serde_json::Value::Array(entries),
                });
            }

            if let Err(e) = coveflow_queue::append_run_log_chunks(&self.db, &chunks).await {
                eprintln!("[DbLogLayer] failed to append run log chunks: {e}");
            }
        }

        // Build and insert service log chunks
        if !service_events.is_empty() {
            let seq = self.service_seq.fetch_add(1, Ordering::Relaxed);

            let mut min_level: i16 = i16::MAX;
            let mut max_level: i16 = i16::MIN;
            let mut entries = Vec::new();

            // Use instance_id/service from first event (they're all the same)
            let instance_id = service_events[0].instance_id.clone();
            let service = service_events[0].service.clone();

            for event in &service_events {
                let lvl = level_to_i16(&event.level);
                min_level = min_level.min(lvl);
                max_level = max_level.max(lvl);

                let mut entry = serde_json::json!({
                    "ts": event.timestamp.to_rfc3339(),
                    "level": lvl,
                    "msg": event.message,
                    "target": event.target,
                });

                if let serde_json::Value::Object(ref fields) = event.fields {
                    if !fields.is_empty() {
                        entry["fields"] = event.fields.clone();
                    }
                }

                entries.push(entry);
            }

            let chunks = [ServiceLogChunk {
                instance_id,
                service,
                seq,
                min_level,
                max_level,
                line_count: entries.len() as i16,
                entries: serde_json::Value::Array(entries),
            }];

            if let Err(e) = coveflow_queue::append_service_log_chunks(&self.db, &chunks).await {
                eprintln!("[DbLogLayer] failed to append service log chunks: {e}");
            }
        }

        buffer.clear();
    }
}

/// Initialize the DbLogLayer and spawn the background LogFlusher task.
///
/// Returns the layer to be added to a tracing subscriber.
/// The LogFlusher runs as a background tokio task and will drain
/// remaining events when the layer is dropped (channel closes).
pub fn init_db_log_layer(db: PgPool, instance_id: String, service: String) -> DbLogLayer {
    let (tx, rx) = mpsc::channel(10_000);
    let dropped = Arc::new(AtomicU64::new(0));

    let flusher = LogFlusher::new(rx, db);
    tokio::spawn(flusher.run());

    DbLogLayer {
        tx,
        instance_id,
        service,
        dropped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_level_to_i16() {
        assert_eq!(level_to_i16(&tracing::Level::TRACE), 1);
        assert_eq!(level_to_i16(&tracing::Level::DEBUG), 2);
        assert_eq!(level_to_i16(&tracing::Level::INFO), 3);
        assert_eq!(level_to_i16(&tracing::Level::WARN), 4);
        assert_eq!(level_to_i16(&tracing::Level::ERROR), 5);
    }

    #[test]
    fn test_level_ordering() {
        // Ensure levels are monotonically increasing in severity
        let levels = [
            tracing::Level::TRACE,
            tracing::Level::DEBUG,
            tracing::Level::INFO,
            tracing::Level::WARN,
            tracing::Level::ERROR,
        ];
        for window in levels.windows(2) {
            assert!(
                level_to_i16(&window[0]) < level_to_i16(&window[1]),
                "{:?} should have lower numeric value than {:?}",
                window[0],
                window[1]
            );
        }
    }
}
