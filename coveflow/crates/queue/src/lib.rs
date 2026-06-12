mod cancel;
mod claim;
mod finish;
mod flow;
pub mod log;
mod mark;
mod reap;
mod rerun;
pub mod retention;
mod run_context;
pub mod schedule;
mod submit;
pub mod trigger;

pub use cancel::{CancelOutcome, cancel_run, cancel_run_tree, check_cancel};
pub use claim::{ActiveRun, claim_run, unclaim_run};
pub use finish::finish_run;
pub use flow::{FlowProgress, advance_flow, on_child_complete};
pub use log::{
    RunLogChunk, RunLogChunkRow, ServiceLogChunk, ServiceLogChunkRow, append_run_log_chunks,
    append_service_log_chunks, get_run_log_chunks, get_service_log_chunks, get_service_log_since,
};
pub use mark::{mark_fail, mark_success};
pub use reap::{ReapOutcome, reap_lost_workers};
pub use rerun::{RerunResult, rerun};
pub use retention::{RetentionConfig, RetentionResult, execute_retention, submit_retention_run};
pub use run_context::build_run_context;
pub(crate) use run_context::{ContextParts, build_from_parts};
pub use schedule::{
    ScheduleError, next_after, run_due_schedules, upcoming, validate as validate_cron,
};
pub use submit::{NewRun, submit_run};
pub use trigger::{TriggerError, TriggerKind, WebhookTrigger, submit_triggered_run};

#[derive(Debug, thiserror::Error)]
pub enum QueueError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("quota exceeded: {0}")]
    QuotaExceeded(String),
    #[error("{0}")]
    Other(String),
}

pub type QueueResult<T> = Result<T, QueueError>;
