use crate::QueueResult;
use coveflow_types::ScriptLang;
use coveflow_types::run::{Run, RunKind};
use sqlx::PgPool;

#[derive(Debug)]
pub struct ActiveRun {
    pub run: Run,
    pub tag: String,
    pub cpus: f32,
    pub memory_mb: i32,
    pub disk_mb: i32,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub scheduled_for: chrono::DateTime<chrono::Utc>,
}

#[tracing::instrument(
    name = "queue::claim_run",
    skip(db, tags),
    fields(%worker_name, available_cpus, available_memory_mb, available_disk_mb)
)]
pub async fn claim_run(
    db: &PgPool,
    worker_name: &str,
    tags: &[String],
    available_cpus: f32,
    available_memory_mb: i64,
    available_disk_mb: i64,
) -> QueueResult<Option<ActiveRun>> {
    let row = sqlx::query!(
        r#"
        WITH next_run AS (
            SELECT rq.id, rq.tag, r.cpus, r.memory_mb, r.disk_mb
            FROM run_queue rq
            JOIN run r ON r.id = rq.id
            WHERE rq.running = FALSE
              AND rq.scheduled_for <= now()
              AND rq.canceled_by IS NULL
              AND rq.tag = ANY($1)
              AND r.cpus <= $3
              AND r.memory_mb <= $4
              AND r.disk_mb <= $5
              -- Global concurrent limit (no row in worker_config = unlimited)
              AND COALESCE(
                  (SELECT max_concurrent_runs FROM worker_config LIMIT 1),
                  2147483647
              ) > (SELECT COUNT(*) FROM run_queue WHERE running = TRUE)
              -- L3: Tag-level concurrency control
              AND NOT EXISTS (
                  SELECT 1 FROM concurrency_limit cl
                  WHERE cl.tag = rq.tag
                    AND (SELECT COUNT(*) FROM run_queue rq2
                         WHERE rq2.tag = rq.tag AND rq2.running = TRUE) >= cl.max_concurrent
              )
              -- L5a: Team concurrent run limit
              AND (
                  r.team_owner IS NULL
                  OR NOT EXISTS (
                      SELECT 1 FROM team_quota tq
                      WHERE tq.workspace_id = r.workspace_id
                        AND tq.team_name = r.team_owner
                        AND tq.max_concurrent_runs IS NOT NULL
                        AND (SELECT COUNT(*) FROM run_queue rq3
                             JOIN run r3 ON r3.id = rq3.id
                             WHERE r3.workspace_id = r.workspace_id
                               AND r3.team_owner = r.team_owner
                               AND rq3.running = TRUE) >= tq.max_concurrent_runs
                  )
              )
              -- L5b: Team CPU limit
              AND (
                  r.team_owner IS NULL
                  OR NOT EXISTS (
                      SELECT 1 FROM team_quota tq
                      WHERE tq.workspace_id = r.workspace_id
                        AND tq.team_name = r.team_owner
                        AND tq.max_cpus IS NOT NULL
                        AND (
                            SELECT COALESCE(SUM(r4.cpus::DOUBLE PRECISION), 0)
                            FROM run_queue rq4
                            JOIN run r4 ON r4.id = rq4.id
                            WHERE r4.workspace_id = r.workspace_id
                              AND r4.team_owner = r.team_owner
                              AND rq4.running = TRUE
                        ) + r.cpus::DOUBLE PRECISION > tq.max_cpus::DOUBLE PRECISION
                  )
              )
              -- L5c: Team memory limit
              AND (
                  r.team_owner IS NULL
                  OR NOT EXISTS (
                      SELECT 1 FROM team_quota tq
                      WHERE tq.workspace_id = r.workspace_id
                        AND tq.team_name = r.team_owner
                        AND tq.max_memory_mb IS NOT NULL
                        AND (
                            SELECT COALESCE(SUM(r5.memory_mb), 0)
                            FROM run_queue rq5
                            JOIN run r5 ON r5.id = rq5.id
                            WHERE r5.workspace_id = r.workspace_id
                              AND r5.team_owner = r.team_owner
                              AND rq5.running = TRUE
                        ) + r.memory_mb > tq.max_memory_mb
                  )
              )
            ORDER BY rq.priority DESC, rq.scheduled_for ASC
            LIMIT 1
            FOR UPDATE OF rq SKIP LOCKED
        ),
        claimed AS (
            UPDATE run_queue
            SET running = TRUE, started_at = now(), worker = $2
            FROM next_run
            WHERE run_queue.id = next_run.id
            RETURNING run_queue.id, run_queue.tag
        ),
        ping AS (
            UPDATE worker_ping
            SET current_run_id = claimed.id
            FROM claimed
            WHERE worker_ping.worker = $2
        )
        SELECT claimed.id as "id!",
               claimed.tag as "tag!",
               r.cpus as "cpus!",
               r.memory_mb as "memory_mb!",
               r.disk_mb as "disk_mb!",
               r.created_at as "created_at!",
               r.workspace_id as "workspace_id!",
               r.kind as "kind!: RunKind",
               r.script_hash,
               r.script_path,
               r.raw_code,
               r.language as "language?: ScriptLang",
               r.args,
               r.tag as "run_tag!",
               r.parent_run,
               r.root_run,
               r.requirements as "requirements!",
               r.timeout,
               r.custom_image,
               r.created_by as "created_by!",
               r.trace_id,
               r.span_id,
               rq.scheduled_for as "scheduled_for!"
        FROM claimed
        JOIN run r ON r.id = claimed.id
        JOIN run_queue rq ON rq.id = claimed.id
        "#,
        tags as &[String],
        worker_name,
        available_cpus,
        available_memory_mb as i32,
        available_disk_mb as i32,
    )
    .fetch_optional(db)
    .await?;

    match row {
        Some(r) => {
            tracing::info!(run_id = %r.id, tag = %r.tag, "run claimed");
            Ok(Some(ActiveRun {
                run: Run {
                    id: r.id,
                    workspace_id: r.workspace_id,
                    kind: r.kind,
                    script_hash: r.script_hash,
                    script_path: r.script_path,
                    raw_code: r.raw_code,
                    language: r.language,
                    args: r.args,
                    tag: r.run_tag,
                    parent_run: r.parent_run,
                    root_run: r.root_run,
                    requirements: r.requirements,
                    timeout: r.timeout,
                    custom_image: r.custom_image,
                    created_by: r.created_by,
                    trace_id: r.trace_id,
                    span_id: r.span_id,
                },
                tag: r.tag,
                cpus: r.cpus,
                memory_mb: r.memory_mb,
                disk_mb: r.disk_mb,
                created_at: r.created_at,
                scheduled_for: r.scheduled_for,
            }))
        }
        None => Ok(None),
    }
}

pub async fn unclaim_run(db: &PgPool, run_id: uuid::Uuid, worker_name: &str) -> QueueResult<()> {
    let mut tx = db.begin().await?;

    sqlx::query!(
        "UPDATE run_queue SET running = FALSE, started_at = NULL, worker = NULL WHERE id = $1",
        run_id,
    )
    .execute(&mut *tx)
    .await?;

    sqlx::query!(
        "UPDATE worker_ping SET current_run_id = NULL WHERE worker = $1 AND current_run_id = $2",
        worker_name,
        run_id,
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    tracing::info!(run_id = %run_id, "run unclaimed and returned to queue");
    Ok(())
}
