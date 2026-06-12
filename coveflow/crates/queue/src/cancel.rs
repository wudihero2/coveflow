use crate::QueueResult;
use sqlx::PgPool;
use uuid::Uuid;

/// Outcome of a cancel request.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CancelOutcome {
    /// Run was completed immediately as failed (queued, or force mode).
    CompletedImmediately,
    /// Run is currently running; cancel flag has been set. Worker will detect it.
    FlagSet,
    /// Run was already completed (in run_completed) before we could cancel.
    AlreadyCompleted,
    /// Run ID does not exist.
    NotFound,
}

/// Cancel a run.
///
/// **Soft cancel** (`force = false`):
/// - Queued (not running) → complete immediately as failed.
/// - Running → set cancel flag; worker detects via `LISTEN 'run_cancel'` or polling.
///
/// **Force cancel** (`force = true`):
/// - Directly mark as failed regardless of running state.
/// - Used for zombie runs, dead workers, or stuck processes.
#[tracing::instrument(
    name = "queue::cancel_run",
    skip(db),
    fields(%run_id, %canceled_by, force)
)]
pub async fn cancel_run(
    db: &PgPool,
    run_id: Uuid,
    canceled_by: &str,
    reason: Option<&str>,
    force: bool,
) -> QueueResult<CancelOutcome> {
    let mut tx = db.begin().await?;

    // Lock the run_queue row (if it exists)
    let row = sqlx::query!(
        "SELECT running FROM run_queue WHERE id = $1 FOR UPDATE",
        run_id
    )
    .fetch_optional(&mut *tx)
    .await?;

    let Some(row) = row else {
        // Not in queue — check if already completed or doesn't exist
        let completed = sqlx::query_scalar!(
            r#"SELECT EXISTS(SELECT 1 FROM run_completed WHERE id = $1) as "exists!""#,
            run_id
        )
        .fetch_one(&mut *tx)
        .await?;

        let run_exists = sqlx::query_scalar!(
            r#"SELECT EXISTS(SELECT 1 FROM run WHERE id = $1) as "exists!""#,
            run_id
        )
        .fetch_one(&mut *tx)
        .await?;

        tx.commit().await?;

        return if completed || run_exists {
            Ok(CancelOutcome::AlreadyCompleted)
        } else {
            Ok(CancelOutcome::NotFound)
        };
    };

    // If not running, or force mode → complete immediately
    if !row.running || force {
        let message = if force {
            "force canceled"
        } else {
            "canceled before execution"
        };

        sqlx::query!(
            "INSERT INTO run_completed (id, success, result, duration_ms, memory_peak_bytes, canceled_by, canceled_reason)
             VALUES ($1, FALSE, jsonb_build_object('error', jsonb_build_object('message', $4::text)), 0, 0, $2, $3)",
            run_id,
            canceled_by,
            reason,
            message,
        )
        .execute(&mut *tx)
        .await?;

        sqlx::query!("DELETE FROM run_queue WHERE id = $1", run_id)
            .execute(&mut *tx)
            .await?;

        sqlx::query!(
            "UPDATE worker_ping SET current_run_id = NULL, runs_completed = runs_completed + 1
             WHERE current_run_id = $1",
            run_id
        )
        .execute(&mut *tx)
        .await
        .ok();

        tx.commit().await?;

        tracing::info!(run_id = %run_id, force, "run canceled immediately");
        return Ok(CancelOutcome::CompletedImmediately);
    }

    // Running (soft cancel) — set cancel flag for worker to detect
    let now = chrono::Utc::now();

    sqlx::query!(
        "UPDATE run_queue SET canceled_by = $2, canceled_reason = $3, cancel_requested_at = $4
         WHERE id = $1",
        run_id,
        canceled_by,
        reason,
        now,
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    tracing::info!(run_id = %run_id, "cancel flag set for running run");
    Ok(CancelOutcome::FlagSet)
}

/// Recursively cancel a run and all its descendant (child) runs.
/// Used when canceling a flow to also cancel all sub-runs.
#[tracing::instrument(
    name = "queue::cancel_run_tree",
    skip(db),
    fields(%run_id, %canceled_by)
)]
pub async fn cancel_run_tree(
    db: &PgPool,
    run_id: Uuid,
    canceled_by: &str,
    reason: Option<&str>,
) -> QueueResult<Vec<(Uuid, CancelOutcome)>> {
    let descendants = sqlx::query_scalar!(
        r#"
        WITH RECURSIVE descendants AS (
            SELECT id FROM run WHERE id = $1
            UNION ALL
            SELECT r.id FROM run r JOIN descendants d ON r.parent_run = d.id
        )
        SELECT id as "id!" FROM descendants
        "#,
        run_id
    )
    .fetch_all(db)
    .await?;

    let mut results = Vec::with_capacity(descendants.len());
    for desc_id in descendants {
        let outcome = cancel_run(db, desc_id, canceled_by, reason, false).await?;
        results.push((desc_id, outcome));
    }

    tracing::info!(
        run_id = %run_id,
        total_canceled = results.len(),
        "run tree cancel complete"
    );

    Ok(results)
}

/// Check if a run has been canceled. Used by worker's cancel detection loop.
#[tracing::instrument(
    name = "queue::check_cancel",
    skip(db),
    fields(%run_id)
)]
pub async fn check_cancel(
    db: &PgPool,
    run_id: Uuid,
) -> QueueResult<Option<(String, Option<String>)>> {
    let row = sqlx::query!(
        "SELECT canceled_by, canceled_reason FROM run_queue WHERE id = $1",
        run_id
    )
    .fetch_optional(db)
    .await?;

    match row {
        Some(r) => match r.canceled_by {
            Some(by) => Ok(Some((by, r.canceled_reason))),
            None => Ok(None),
        },
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cancel_outcome_serialization() {
        assert!(matches!(
            serde_json::to_string(&CancelOutcome::CompletedImmediately).as_deref(),
            Ok("\"completed_immediately\"")
        ));
        assert!(matches!(
            serde_json::to_string(&CancelOutcome::FlagSet).as_deref(),
            Ok("\"flag_set\"")
        ));
        assert!(matches!(
            serde_json::to_string(&CancelOutcome::AlreadyCompleted).as_deref(),
            Ok("\"already_completed\"")
        ));
        assert!(matches!(
            serde_json::to_string(&CancelOutcome::NotFound).as_deref(),
            Ok("\"not_found\"")
        ));
    }
}
