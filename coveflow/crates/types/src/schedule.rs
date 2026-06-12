//! Cron schedule model. A `Schedule` triggers a flow on a cron
//! expression. Flows are referenced by stable `flow_id`; the current path is
//! resolved on demand (the path is a movable label).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A cron schedule that fires a flow. Mirrors the `schedule` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Schedule {
    pub id: Uuid,
    pub workspace_id: String,
    pub name: String,
    /// The flow this schedule triggers, by stable id (path is resolved on demand).
    pub flow_id: Uuid,
    /// Standard 5/6-field cron expression, e.g. `0 2 * * 1-5`.
    pub cron_expr: String,
    /// IANA timezone the cron is evaluated in, e.g. `Asia/Taipei`.
    pub timezone: String,
    /// Arguments passed as the flow's `flow.input`.
    pub args: serde_json::Value,
    pub enabled: bool,
    /// When true, missed ticks (e.g. after downtime) are backfilled, capped per
    /// scheduler pass. When false, only the next upcoming tick fires.
    pub catchup: bool,
    /// Max concurrent (non-terminal) runs for this schedule: `None` = unlimited,
    /// `1` = no overlap (wait for previous), `N` = at most N.
    pub max_active_runs: Option<i32>,
    pub next_trigger_at: Option<DateTime<Utc>>,
    pub last_triggered_at: Option<DateTime<Utc>>,
    /// Why the last tick did not fire (e.g. flow missing), shown in the UI.
    pub last_error: Option<String>,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
