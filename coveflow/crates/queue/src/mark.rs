use crate::QueueResult;
use sqlx::PgPool;
use uuid::Uuid;

/// Mark a run as successful, regardless of its current state.
/// Requires admin privileges (enforced at API layer).
#[tracing::instrument(
    name = "queue::mark_success",
    skip(db, result),
    fields(%run_id, %marked_by)
)]
pub async fn mark_success(
    db: &PgPool,
    run_id: Uuid,
    marked_by: &str,
    reason: Option<&str>,
    result: Option<serde_json::Value>,
) -> QueueResult<()> {
    mark_run(db, run_id, true, marked_by, reason, result).await
}

/// Mark a run as failed, regardless of its current state.
/// Requires admin privileges (enforced at API layer).
#[tracing::instrument(
    name = "queue::mark_fail",
    skip(db, result),
    fields(%run_id, %marked_by)
)]
pub async fn mark_fail(
    db: &PgPool,
    run_id: Uuid,
    marked_by: &str,
    reason: Option<&str>,
    result: Option<serde_json::Value>,
) -> QueueResult<()> {
    mark_run(db, run_id, false, marked_by, reason, result).await
}

async fn mark_run(
    db: &PgPool,
    run_id: Uuid,
    success: bool,
    marked_by: &str,
    reason: Option<&str>,
    result: Option<serde_json::Value>,
) -> QueueResult<()> {
    let mut tx = db.begin().await?;

    let label = if success { "success" } else { "failure" };
    let result_value = result.unwrap_or(serde_json::json!({"marked": label}));

    sqlx::query!(
        "INSERT INTO run_completed (id, success, result, duration_ms, memory_peak_bytes, marked_by, mark_reason)
         VALUES ($1, $5, $2, 0, 0, $3, $4)
         ON CONFLICT (id) DO UPDATE SET success = $5, marked_by = $3, mark_reason = $4",
        run_id,
        result_value,
        marked_by,
        reason,
        success,
    )
    .execute(&mut *tx)
    .await?;

    // Remove from queue if still there
    sqlx::query!("DELETE FROM run_queue WHERE id = $1", run_id)
        .execute(&mut *tx)
        .await?;

    // Clear worker_ping
    sqlx::query!(
        "UPDATE worker_ping SET current_run_id = NULL, runs_completed = runs_completed + 1
         WHERE current_run_id = $1",
        run_id
    )
    .execute(&mut *tx)
    .await
    .ok();

    tx.commit().await?;

    tracing::info!(run_id = %run_id, "run marked as {label}");
    Ok(())
}
