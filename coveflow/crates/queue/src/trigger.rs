//! Trigger framework: the shared "event → flow run" submit path.
//!
//! A trigger fires a flow run via a type-specific path (v1: webhook = inbound
//! HTTP). Firing is type-specific, but every type converges on
//! [`submit_triggered_run`], which snapshots the flow, links the run to the
//! trigger (`run.trigger_id`), records provenance (`run.trigger_context` →
//! `ctx.trigger`), and enforces the trigger's `max_active_runs`.

use coveflow_types::RunKind;
use coveflow_types::trigger::{TriggerRow, WEBHOOK_TYPE};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::submit::submit_run_tx;
use crate::{NewRun, QueueError};

#[derive(Debug, thiserror::Error)]
pub enum TriggerError {
    #[error("invalid trigger config: {0}")]
    InvalidConfig(String),
    #[error("flow {0} not found")]
    FlowNotFound(Uuid),
    #[error("trigger '{0}' is at its max active runs limit")]
    MaxActiveRuns(String),
    #[error(transparent)]
    Queue(#[from] QueueError),
}

/// Type-specific trigger behavior. Adding a new push trigger type = implement
/// this + register its firing path; all converge on [`submit_triggered_run`].
pub trait TriggerKind {
    fn type_name() -> &'static str;
    /// Validate the type-specific `config` JSON at create/update time.
    fn validate_config(config: &Value) -> Result<(), TriggerError>;
}

/// The webhook trigger type. Config: `{ "max_active_runs": int? }`.
pub struct WebhookTrigger;

#[derive(serde::Deserialize)]
struct WebhookConfig {
    #[serde(default)]
    max_active_runs: Option<i32>,
}

impl TriggerKind for WebhookTrigger {
    fn type_name() -> &'static str {
        WEBHOOK_TYPE
    }

    fn validate_config(config: &Value) -> Result<(), TriggerError> {
        let cfg: WebhookConfig = serde_json::from_value(config.clone())
            .map_err(|e| TriggerError::InvalidConfig(e.to_string()))?;
        if matches!(cfg.max_active_runs, Some(n) if n < 1) {
            return Err(TriggerError::InvalidConfig(
                "max_active_runs must be >= 1".into(),
            ));
        }
        Ok(())
    }
}

/// Submit a flow run fired by `trigger`, executing as `run_as`. Steps:
/// 1. (in one tx) enforce `max_active_runs`,
/// 2. snapshot the flow's latest revision (resolving its current path),
/// 3. submit the run linked to the trigger with `input` + provenance.
///
/// The max-active-runs check is best-effort under concurrency (one tx, no row
/// lock) — same guarantee cron gives per-tick.
#[tracing::instrument(
    name = "queue::submit_triggered_run",
    skip(db, input, trigger_context),
    fields(trigger_id = %trigger.id, %run_as)
)]
pub async fn submit_triggered_run(
    db: &PgPool,
    trigger: &TriggerRow,
    run_as: &str,
    input: Value,
    trigger_context: Value,
) -> Result<Uuid, TriggerError> {
    let mut tx = db.begin().await.map_err(QueueError::from)?;

    if let Some(max) = max_active_runs(&trigger.config) {
        let active = sqlx::query_scalar!(
            r#"SELECT count(*) AS "n!" FROM run r
               WHERE r.trigger_id = $1
                 AND NOT EXISTS (SELECT 1 FROM run_completed c WHERE c.id = r.id)"#,
            trigger.id
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(QueueError::from)?;
        if active >= max as i64 {
            return Err(TriggerError::MaxActiveRuns(trigger.name.clone()));
        }
    }

    let flow = sqlx::query!(
        "SELECT value, path FROM flow WHERE workspace_id = $1 AND flow_id = $2
         ORDER BY revision DESC LIMIT 1",
        trigger.workspace_id,
        trigger.flow_id
    )
    .fetch_optional(&mut *tx)
    .await
    .map_err(QueueError::from)?
    .ok_or(TriggerError::FlowNotFound(trigger.flow_id))?;

    let run_id = submit_run_tx(
        &mut tx,
        NewRun {
            workspace_id: &trigger.workspace_id,
            kind: RunKind::Flow,
            script_hash: None,
            script_path: Some(&flow.path),
            raw_code: None,
            language: None,
            args: Some(input),
            flow_value: Some(flow.value),
            tag: "default",
            parent_run: None,
            root_run: None,
            flow_step_id: None,
            team_owner: None,
            created_by: run_as,
            trace_id: None,
            span_id: None,
            scheduled_for: None,
            priority: None,
            cpus: None,
            memory_mb: None,
            disk_mb: None,
            requirements: vec![],
            timeout: None,
            custom_image: None,
            schedule_id: None,
            scheduled_time: None,
            data_interval_end: None,
            trigger_id: Some(trigger.id),
            trigger_context: Some(trigger_context),
        },
    )
    .await?;

    tx.commit().await.map_err(QueueError::from)?;
    Ok(run_id)
}

fn max_active_runs(config: &Value) -> Option<i32> {
    config
        .get("max_active_runs")
        .and_then(|v| v.as_i64())
        .map(|n| n as i32)
}
