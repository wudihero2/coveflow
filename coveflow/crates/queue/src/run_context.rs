//! Builder for a run's Airflow-style execution context ([`RunContext`]).
//!
//! `build_run_context` derives the context from three sources in one read pass:
//! the run row, its root flow run (interval / schedule are inherited by flow
//! child runs), and the triggering schedule (name + timezone). It is pure: the
//! worker calls it once before executing a script, and `get_run` calls it to
//! display the context — the same builder powers both.

use chrono::{DateTime, SecondsFormat, Utc};
use coveflow_types::flow_status::FlowRunState;
use coveflow_types::run_context::RunContext;
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::{QueueError, QueueResult};

/// Derive the execution context for `run_id`. Steps to read:
/// 1. fetch the run's metadata (joined with root run + schedule),
/// 2. fetch the upstream step results from the owning flow's status,
/// 3. assemble the timestamps / identity / schedule fields.
#[tracing::instrument(name = "queue::build_run_context", skip(db))]
pub async fn build_run_context(db: &PgPool, run_id: Uuid) -> QueueResult<RunContext> {
    let meta = fetch_meta(db, run_id).await?;
    // Steps come from the owning flow's status: the root flow run for a child,
    // or the run itself for a flow run. A standalone script has no flow_status row.
    let steps = fetch_steps(db, meta.root_run.unwrap_or(run_id)).await?;
    Ok(assemble(run_id, meta, steps))
}

/// Run metadata after COALESCE-ing inherited fields from the root flow run and
/// joining the triggering schedule. All times are still `Option` here; the
/// fallbacks to `created_at` / `UTC` happen in [`assemble`].
struct Meta {
    created_by: String,
    created_at: DateTime<Utc>,
    root_run: Option<Uuid>,
    logical_date: Option<DateTime<Utc>>,
    interval_end: Option<DateTime<Utc>>,
    schedule_id: Option<Uuid>,
    flow_path: Option<String>,
    flow_input: Option<Value>,
    schedule_name: Option<String>,
    schedule_tz: Option<String>,
    trigger_context: Option<Value>,
}

async fn fetch_meta(db: &PgPool, run_id: Uuid) -> QueueResult<Meta> {
    let row = sqlx::query!(
        r#"SELECT
            r.created_by,
            r.created_at,
            r.root_run,
            COALESCE(r.scheduled_time, root.scheduled_time)       AS logical_date,
            COALESCE(r.data_interval_end, root.data_interval_end) AS interval_end,
            COALESCE(r.schedule_id, root.schedule_id)             AS schedule_id,
            COALESCE(root.script_path,
                     CASE WHEN r.kind = 'flow' THEN r.script_path END) AS flow_path,
            root.args                                             AS flow_input,
            COALESCE(r.trigger_context, root.trigger_context)     AS trigger_context,
            s.name                                                AS "schedule_name?",
            s.timezone                                            AS "schedule_tz?"
        FROM run r
        LEFT JOIN run root ON root.id = r.root_run AND root.id <> r.id
        LEFT JOIN schedule s ON s.id = COALESCE(r.schedule_id, root.schedule_id)
        WHERE r.id = $1"#,
        run_id
    )
    .fetch_optional(db)
    .await?
    .ok_or_else(|| QueueError::Other(format!("run {run_id} not found")))?;

    Ok(Meta {
        created_by: row.created_by,
        created_at: row.created_at,
        root_run: row.root_run,
        logical_date: row.logical_date,
        interval_end: row.interval_end,
        schedule_id: row.schedule_id,
        flow_path: row.flow_path,
        flow_input: row.flow_input,
        schedule_name: row.schedule_name,
        schedule_tz: row.schedule_tz,
        trigger_context: row.trigger_context,
    })
}

/// Read the succeeded upstream node results from `run_flow_status`. Returns an
/// empty object when there is no flow status (a standalone script run).
async fn fetch_steps(db: &PgPool, status_run_id: Uuid) -> QueueResult<Value> {
    let raw = sqlx::query_scalar!(
        "SELECT flow_status FROM run_flow_status WHERE run_id = $1",
        status_run_id
    )
    .fetch_optional(db)
    .await?;

    let Some(raw) = raw else {
        return Ok(Value::Object(Default::default()));
    };
    let state: FlowRunState = serde_json::from_value(raw)
        .map_err(|e| QueueError::Other(format!("invalid flow_status: {e}")))?;
    Ok(Value::Object(state.succeeded_steps()))
}

fn assemble(run_id: Uuid, meta: Meta, steps: Value) -> RunContext {
    build_from_parts(ContextParts {
        run_id,
        flow_run_id: meta.root_run,
        flow_path: meta.flow_path,
        created_by: meta.created_by,
        created_at: meta.created_at,
        logical_date: meta.logical_date,
        interval_end: meta.interval_end,
        schedule_id: meta.schedule_id,
        schedule_name: meta.schedule_name,
        schedule_tz: meta.schedule_tz,
        flow_input: meta.flow_input,
        steps,
        trigger: meta.trigger_context,
    })
}

/// Raw inputs to [`build_from_parts`]. `logical_date` / `interval_end` /
/// `schedule_tz` are still optional here so the single assembler owns the
/// manual-run fallbacks (created_at, zero-width interval, UTC).
pub(crate) struct ContextParts {
    pub run_id: Uuid,
    pub flow_run_id: Option<Uuid>,
    pub flow_path: Option<String>,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
    pub logical_date: Option<DateTime<Utc>>,
    pub interval_end: Option<DateTime<Utc>>,
    pub schedule_id: Option<Uuid>,
    pub schedule_name: Option<String>,
    pub schedule_tz: Option<String>,
    pub flow_input: Option<Value>,
    pub steps: Value,
    /// Trigger provenance (`run.trigger_context`); `None` for non-trigger runs.
    pub trigger: Option<Value>,
}

/// The single place that turns raw run data into a [`RunContext`]: applies the
/// manual-run fallbacks and formats every timestamp. Shared by the DB-backed
/// builder and the in-engine flow builder so both produce identical context.
pub(crate) fn build_from_parts(p: ContextParts) -> RunContext {
    // Manual runs have no scheduled slot: logical date falls back to created_at
    // and the interval collapses to zero width at that instant.
    let logical = p.logical_date.unwrap_or(p.created_at);
    let interval_end = p.interval_end.unwrap_or(logical);
    let timezone = p.schedule_tz.unwrap_or_else(|| "UTC".to_string());
    let (ds, ts) = local_ds_ts(logical, &timezone);

    RunContext {
        data_interval_start: utc_str(logical),
        data_interval_end: utc_str(interval_end),
        logical_date: utc_str(logical),
        ds,
        ts,
        timezone,
        run_id: p.run_id.to_string(),
        flow_run_id: p.flow_run_id.map(|id| id.to_string()),
        flow_path: p.flow_path,
        created_by: p.created_by,
        is_scheduled: p.schedule_id.is_some(),
        schedule_id: p.schedule_id.map(|id| id.to_string()),
        schedule_name: p.schedule_name,
        triggered_at: utc_str(p.created_at),
        flow_input: p.flow_input,
        steps: p.steps,
        trigger: p.trigger,
    }
}

/// RFC3339 with a `Z` suffix (seconds precision) — the canonical UTC form used
/// for every absolute timestamp in the context.
fn utc_str(dt: DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// `(ds, ts)` of `logical` rendered in `tz_name` (the schedule timezone). `ds`
/// is the calendar date, `ts` is RFC3339 with the zone offset. An unknown zone
/// falls back to UTC.
fn local_ds_ts(logical: DateTime<Utc>, tz_name: &str) -> (String, String) {
    match tz_name.parse::<chrono_tz::Tz>() {
        Ok(tz) => {
            let local = logical.with_timezone(&tz);
            (
                local.format("%Y-%m-%d").to_string(),
                local.to_rfc3339_opts(SecondsFormat::Secs, false),
            )
        }
        Err(_) => (logical.format("%Y-%m-%d").to_string(), utc_str(logical)),
    }
}
