//! Liveness reaper for lost workers.
//!
//! Workers heartbeat into `worker_ping` every ~30s. When a worker dies (crash,
//! scale-down, network partition that never recovers) its row goes stale and
//! any jobs it had claimed sit in `run_queue` with `running = TRUE` forever —
//! permanently consuming tag/team concurrency budget and showing up as phantom
//! load. Nothing else reclaims them.
//!
//! `reap_lost_workers` runs periodically (an API-side background loop) and, for
//! workers whose heartbeat is older than the reap threshold:
//!   1. Marks their still-running jobs as failed ("worker lost"). We choose
//!      at-most-once (fail, not requeue): user scripts have side effects, so a
//!      job that finished its work but died before `finish_run` must not silently
//!      re-execute. Operators rerun explicitly if they want to.
//!   2. Deletes the stale `worker_ping` rows so the table stays bounded.

use sqlx::PgPool;

use crate::QueueResult;

/// What a single reaper pass did. Logged each tick so operators can see churn.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ReapOutcome {
    /// Running jobs marked failed because their worker was lost.
    pub runs_failed: u64,
    /// Stale `worker_ping` rows removed.
    pub workers_removed: u64,
}

/// Fail jobs owned by lost workers and delete the stale worker rows.
///
/// A worker is "lost" when it has no `worker_ping` row with `ping_at` within
/// `stale_after_secs`. The threshold must be comfortably larger than the
/// heartbeat interval (and the UI's stale badge window) so a brief blip or GC
/// pause does not reap a worker that is still alive and running a long job —
/// the heartbeat keeps `ping_at` fresh regardless of job duration.
///
/// Both steps run in one transaction and are idempotent: failing a job uses
/// `ON CONFLICT DO NOTHING` so a late `finish_run` from a recovered worker
/// cannot collide, and the `worker_ping` delete is naturally repeatable. Two
/// API replicas sweeping concurrently is therefore safe.
#[tracing::instrument(name = "queue::reap_lost_workers", skip(db), fields(stale_after_secs))]
pub async fn reap_lost_workers(db: &PgPool, stale_after_secs: i64) -> QueueResult<ReapOutcome> {
    let mut tx = db.begin().await?;

    let runs_failed = sqlx::query_scalar!(
        r#"
        WITH lost AS (
            DELETE FROM run_queue q
            WHERE q.running = TRUE
              AND q.worker IS NOT NULL
              AND NOT EXISTS (
                  SELECT 1 FROM worker_ping w
                  WHERE w.worker = q.worker
                    AND w.ping_at > now() - make_interval(secs => $1)
              )
            RETURNING q.id, q.canceled_by, q.canceled_reason
        ),
        completed AS (
            INSERT INTO run_completed
                (id, success, result, duration_ms, memory_peak_bytes,
                 canceled_by, canceled_reason)
            SELECT id, FALSE, '{"error":"worker lost"}'::jsonb, 0, NULL::bigint,
                   canceled_by, canceled_reason
            FROM lost
            ON CONFLICT (id) DO NOTHING
            RETURNING id
        )
        SELECT count(*) AS "n!" FROM completed
        "#,
        stale_after_secs as f64,
    )
    .fetch_one(&mut *tx)
    .await?;

    let workers_removed = sqlx::query_scalar!(
        r#"
        WITH gone AS (
            DELETE FROM worker_ping
            WHERE ping_at <= now() - make_interval(secs => $1)
            RETURNING worker
        )
        SELECT count(*) AS "n!" FROM gone
        "#,
        stale_after_secs as f64,
    )
    .fetch_one(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(ReapOutcome {
        runs_failed: runs_failed as u64,
        workers_removed: workers_removed as u64,
    })
}
