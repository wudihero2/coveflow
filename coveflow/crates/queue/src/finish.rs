use crate::QueueResult;
use sqlx::PgPool;
use uuid::Uuid;

#[tracing::instrument(
    name = "queue::finish_run",
    skip(db, result),
    fields(%run_id, success, duration_ms)
)]
pub async fn finish_run(
    db: &PgPool,
    run_id: Uuid,
    success: bool,
    result: serde_json::Value,
    duration_ms: i32,
    memory_peak: i64,
    s3_key: Option<&str>,
) -> QueueResult<()> {
    let mut tx = db.begin().await?;

    // Remove from queue first, capturing any soft-cancel metadata. If the
    // worker was killed mid-execution by a cancel request, these fields tell
    // the API/UI who/why so derive_status can correctly report "cancelled"
    // instead of a generic "failure".
    let queue_row = sqlx::query!(
        "DELETE FROM run_queue WHERE id = $1
         RETURNING canceled_by, canceled_reason",
        run_id,
    )
    .fetch_optional(&mut *tx)
    .await?;

    // Only propagate cancel metadata on failed runs. A run that succeeded
    // before the cancel signal reached the sandbox should still report as
    // "success", not "cancelled".
    let (canceled_by, canceled_reason) = if success {
        (None, None)
    } else {
        queue_row
            .map(|r| (r.canceled_by, r.canceled_reason))
            .unwrap_or((None, None))
    };

    // Insert completion record
    // If s3_key is present, result is stored externally — don't inline it
    let inline_result: Option<serde_json::Value> =
        if s3_key.is_some() { None } else { Some(result) };

    // ON CONFLICT DO NOTHING: the liveness reaper may have already written a
    // terminal "worker lost" row for this run (worker was network-partitioned,
    // declared dead, then recovered and called finish_run late). First writer
    // wins; the late completion is dropped rather than colliding on the PK.
    sqlx::query!(
        "INSERT INTO run_completed (id, success, result, result_s3_key,
                                     duration_ms, memory_peak_bytes,
                                     canceled_by, canceled_reason)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         ON CONFLICT (id) DO NOTHING",
        run_id,
        success,
        inline_result as Option<serde_json::Value>,
        s3_key,
        duration_ms,
        memory_peak,
        canceled_by,
        canceled_reason,
    )
    .execute(&mut *tx)
    .await?;

    // Clear worker's current_run_id and increment completed count
    sqlx::query!(
        "UPDATE worker_ping SET current_run_id = NULL, runs_completed = runs_completed + 1
         WHERE current_run_id = $1",
        run_id
    )
    .execute(&mut *tx)
    .await
    .ok();

    tx.commit().await?;

    tracing::info!(run_id = %run_id, success, duration_ms, "run finished");

    // If this run is a step of a flow, wake the parent flow so it advances.
    // Best-effort: a failed wake is logged, not propagated (the liveness reaper
    // and re-claims provide a backstop).
    if let Err(e) = crate::on_child_complete(db, run_id).await {
        tracing::warn!(error = %e, run_id = %run_id, "on_child_complete (flow wake) failed");
    }

    Ok(())
}
