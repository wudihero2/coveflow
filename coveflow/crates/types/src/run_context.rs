//! Airflow-style execution context for a run.
//!
//! `RunContext` is the single `ctx` dict a script receives (signature-aware: the
//! worker's python wrapper only passes it when `main` declares `ctx` or `**kwargs`)
//! and the value behind the flow expression `run.*` namespace. Every timestamp is
//! a string so the struct is JSON-friendly and directly usable from python.
//!
//! It is derived by `coveflow_queue::build_run_context` (from the run + its root
//! flow run + the triggering schedule) or assembled in-engine from a flow run row.

use serde::{Deserialize, Serialize};

/// One run's execution context. Grouped into time/interval, run identity,
/// schedule source, and inputs/upstream results.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunContext {
    // --- time / interval (RFC3339 UTC strings; `ds`/`ts` in the schedule tz) ---
    /// Start of the data interval = `logical_date`.
    pub data_interval_start: String,
    /// End of the data interval (next cron slot); equals `logical_date` for
    /// manual runs (zero-width interval).
    pub data_interval_end: String,
    /// The cron slot this run represents (= `data_interval_start`).
    pub logical_date: String,
    /// `logical_date` as a date (`%Y-%m-%d`) in the schedule timezone.
    pub ds: String,
    /// `logical_date` as RFC3339 with offset in the schedule timezone.
    pub ts: String,
    /// Schedule timezone (IANA name); `UTC` for manual runs.
    pub timezone: String,

    // --- run identity ---
    pub run_id: String,
    /// Top-level flow run id; `None` for a standalone (non-flow) script run.
    pub flow_run_id: Option<String>,
    /// Flow path; `None` for a standalone script run.
    pub flow_path: Option<String>,
    pub created_by: String,

    // --- schedule source ---
    pub is_scheduled: bool,
    pub schedule_id: Option<String>,
    pub schedule_name: Option<String>,
    /// Wall-clock time the run was actually created (`run.created_at`).
    pub triggered_at: String,

    // --- inputs + upstream results (~ Airflow params + xcom, flattened) ---
    /// The flow run's input/params; `None` for a standalone script run.
    pub flow_input: Option<serde_json::Value>,
    /// Succeeded upstream node results as `{ <id>: { "result": ... } }`; an empty
    /// object for a non-flow run.
    pub steps: serde_json::Value,

    // --- trigger provenance ---
    /// How this run was triggered, if by a trigger (webhook: `{ type, trigger_id,
    /// source_ip, method, ... }`). `None` for manual / cron / flow-child runs.
    pub trigger: Option<serde_json::Value>,
}
