use crate::{QueueError, QueueResult};
use coveflow_types::{RunKind, ScriptLang};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

pub struct NewRun<'a> {
    pub workspace_id: &'a str,
    pub kind: RunKind,
    pub script_hash: Option<&'a str>,
    pub script_path: Option<&'a str>,
    pub raw_code: Option<&'a str>,
    pub language: Option<ScriptLang>,
    pub args: Option<serde_json::Value>,
    /// Flow definition (only for kind=flow/flow_preview); serialized to run.flow_value.
    pub flow_value: Option<serde_json::Value>,
    pub tag: &'a str,
    pub parent_run: Option<Uuid>,
    pub root_run: Option<Uuid>,
    pub flow_step_id: Option<&'a str>,
    pub team_owner: Option<&'a str>,
    pub created_by: &'a str,
    pub trace_id: Option<&'a str>,
    pub span_id: Option<&'a str>,
    pub scheduled_for: Option<chrono::DateTime<chrono::Utc>>,
    pub priority: Option<i16>,
    pub cpus: Option<f32>,
    pub memory_mb: Option<i32>,
    pub disk_mb: Option<i32>,
    pub requirements: Vec<String>,
    pub timeout: Option<i32>,
    pub custom_image: Option<&'a str>,
    /// The schedule that triggered this run (cron), if any. Powers per-schedule
    /// run history + max_active_runs counting.
    pub schedule_id: Option<Uuid>,
    /// The cron occurrence ("logical date") this run represents, for scheduled
    /// runs. Distinct from when it actually ran. None for manual/ad-hoc runs.
    pub scheduled_time: Option<chrono::DateTime<chrono::Utc>>,
    /// End of the data interval (next cron occurrence after `scheduled_time`),
    /// snapshotted at fire time. None for manual/ad-hoc runs. See migration 0019.
    pub data_interval_end: Option<chrono::DateTime<chrono::Utc>>,
    /// The trigger (e.g. webhook) that fired this run, if any. Parallels
    /// `schedule_id`; powers per-trigger history + max_active_runs. See mig 0023.
    pub trigger_id: Option<Uuid>,
    /// Trigger provenance (webhook: method / source_ip / headers summary / time),
    /// surfaced into the run context as `ctx.trigger`. None for non-trigger runs.
    pub trigger_context: Option<serde_json::Value>,
}

#[tracing::instrument(
    name = "queue::submit_run",
    skip(db, run),
    fields(workspace_id = %run.workspace_id, kind = %run.kind, tag = %run.tag)
)]
pub async fn submit_run(db: &PgPool, run: NewRun<'_>) -> QueueResult<Uuid> {
    let mut tx = db.begin().await?;
    let run_id = submit_run_tx(&mut tx, run).await?;
    tx.commit().await?;
    tracing::info!(run_id = %run_id, "run submitted");
    Ok(run_id)
}

/// Insert a run + queue row inside an existing transaction. Lets a caller make
/// the submission atomic with other work in the same tx (e.g. the scheduler
/// advancing `next_trigger_at` together with firing the run).
pub async fn submit_run_tx(
    tx: &mut Transaction<'_, Postgres>,
    run: NewRun<'_>,
) -> QueueResult<Uuid> {
    let run_id = Uuid::new_v4();

    // L5: Team quota checks (if run belongs to a team)
    if let Some(team_owner) = run.team_owner {
        let quota = sqlx::query!(
            "SELECT max_concurrent_runs, max_cpus, max_memory_mb, max_daily_runs
             FROM team_quota
             WHERE workspace_id = $1 AND team_name = $2",
            run.workspace_id,
            team_owner
        )
        .fetch_optional(&mut **tx)
        .await?;

        if let Some(q) = quota {
            // Check concurrent run limit
            if let Some(max_conc) = q.max_concurrent_runs {
                let running = sqlx::query_scalar!(
                    r#"SELECT COUNT(*) as "count!" FROM run_queue rq
                     JOIN run r ON r.id = rq.id
                     WHERE r.workspace_id = $1
                       AND r.team_owner = $2
                       AND rq.running = TRUE"#,
                    run.workspace_id,
                    team_owner
                )
                .fetch_one(&mut **tx)
                .await?;

                if running >= max_conc as i64 {
                    return Err(QueueError::QuotaExceeded(format!(
                        "team '{}' concurrent run limit reached ({}/{})",
                        team_owner, running, max_conc
                    )));
                }
            }

            // Check CPU limit
            if let Some(max_c) = q.max_cpus {
                let used_cpus = sqlx::query_scalar!(
                    r#"SELECT COALESCE(SUM(r.cpus::DOUBLE PRECISION), 0) as "used!" FROM run_queue rq
                     JOIN run r ON r.id = rq.id
                     WHERE r.workspace_id = $1
                       AND r.team_owner = $2
                       AND rq.running = TRUE"#,
                    run.workspace_id,
                    team_owner
                )
                .fetch_one(&mut **tx)
                .await?;

                let needed = run.cpus.unwrap_or(1.0) as f64;
                if used_cpus + needed > max_c as f64 {
                    return Err(QueueError::QuotaExceeded(format!(
                        "team '{}' CPU quota exceeded ({:.1}/{:.1} cpus, needs {:.1})",
                        team_owner, used_cpus, max_c, needed
                    )));
                }
            }

            // Check memory limit
            if let Some(max_m) = q.max_memory_mb {
                let used_mem = sqlx::query_scalar!(
                    r#"SELECT COALESCE(SUM(r.memory_mb), 0) as "used!" FROM run_queue rq
                     JOIN run r ON r.id = rq.id
                     WHERE r.workspace_id = $1
                       AND r.team_owner = $2
                       AND rq.running = TRUE"#,
                    run.workspace_id,
                    team_owner
                )
                .fetch_one(&mut **tx)
                .await?;

                let needed = run.memory_mb.unwrap_or(512) as i64;
                if used_mem + needed > max_m {
                    return Err(QueueError::QuotaExceeded(format!(
                        "team '{}' memory quota exceeded ({}/{} MB, needs {} MB)",
                        team_owner, used_mem, max_m, needed
                    )));
                }
            }

            // Check daily run limit
            if let Some(max_daily) = q.max_daily_runs {
                let today_count = sqlx::query_scalar!(
                    r#"SELECT COUNT(*) as "count!" FROM run r
                     WHERE r.workspace_id = $1
                       AND r.team_owner = $2
                       AND r.created_at >= CURRENT_DATE"#,
                    run.workspace_id,
                    team_owner
                )
                .fetch_one(&mut **tx)
                .await?;

                if today_count >= max_daily as i64 {
                    return Err(QueueError::QuotaExceeded(format!(
                        "team '{}' daily run limit reached ({}/{})",
                        team_owner, today_count, max_daily
                    )));
                }
            }
        }
    }

    // 1. Insert run (immutable definition)
    let cpus = run.cpus.unwrap_or(1.0);
    let memory_mb = run.memory_mb.unwrap_or(512);
    let disk_mb = run.disk_mb.unwrap_or(1024);
    sqlx::query!(
        "INSERT INTO run (id, workspace_id, kind, script_hash, script_path,
         raw_code, language, args, flow_value, tag, parent_run, root_run, flow_step_id,
         cpus, memory_mb, disk_mb, team_owner, created_by, trace_id, span_id,
         requirements, timeout, custom_image, schedule_id, scheduled_time, data_interval_end,
         trigger_id, trigger_context)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23, $24, $25, $26, $27, $28)",
        run_id,
        run.workspace_id,
        run.kind.as_str(),
        run.script_hash,
        run.script_path,
        run.raw_code,
        run.language.as_ref().map(|l| l.as_str()),
        run.args as Option<serde_json::Value>,
        run.flow_value as Option<serde_json::Value>,
        run.tag,
        run.parent_run,
        run.root_run,
        run.flow_step_id,
        cpus,
        memory_mb,
        disk_mb,
        run.team_owner,
        run.created_by,
        run.trace_id,
        run.span_id,
        &run.requirements,
        run.timeout,
        run.custom_image,
        run.schedule_id,
        run.scheduled_time,
        run.data_interval_end,
        run.trigger_id,
        run.trigger_context as Option<serde_json::Value>,
    )
    .execute(&mut **tx)
    .await?;

    // 2. Insert run_queue (mutable scheduling state)
    // Use COALESCE to let PG handle default scheduling time,
    // ensuring clock consistency with claim_run's `now()` check.
    let priority = run.priority.unwrap_or(0);
    sqlx::query!(
        "INSERT INTO run_queue (id, scheduled_for, tag, priority)
         VALUES ($1, COALESCE($2, now()), $3, $4)",
        run_id,
        run.scheduled_for,
        run.tag,
        priority,
    )
    .execute(&mut **tx)
    .await?;

    Ok(run_id)
}
