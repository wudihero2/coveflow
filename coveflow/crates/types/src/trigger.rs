//! Trigger framework types.
//!
//! A `trigger` fires a flow run via a type-specific path. v1 ships `webhook`
//! (inbound HTTP); cron schedules stay in the separate `schedule` table. The
//! shared shape lives here so the API (CRUD) and the queue (`submit_triggered_run`)
//! agree on it.

use serde::Serialize;
use uuid::Uuid;

/// Discriminator stored in `trigger.trigger_type`.
pub const WEBHOOK_TYPE: &str = "webhook";

/// A registered trigger row. `config` is type-specific JSON (webhook:
/// `{ "max_active_runs": int? }`).
#[derive(Debug, Clone, Serialize)]
pub struct TriggerRow {
    pub id: Uuid,
    pub workspace_id: String,
    pub flow_id: Uuid,
    pub trigger_type: String,
    pub name: String,
    pub enabled: bool,
    pub config: serde_json::Value,
    pub created_by: String,
}
