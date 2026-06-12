use crate::{NewRun, QueueError, QueueResult, submit_run};
use coveflow_types::RunKind;
use sqlx::PgPool;
use uuid::Uuid;

const DEFAULT_RETENTION_DAYS: i64 = 30;
const DEFAULT_SERVICE_LOG_RETENTION_DAYS: i64 = 14;
const DEFAULT_BATCH_SIZE: i64 = 5000;
const DEFAULT_BATCH_SLEEP_MS: u64 = 100;
const MAX_RETENTION_DAYS: i64 = 100_000;
const MAX_BATCH_SIZE: i64 = 100_000;

/// Global fallback configuration for log retention.
#[derive(Debug, Clone)]
pub struct RetentionConfig {
    pub default_retention_days: i64,
    pub service_log_retention_days: i64,
    pub batch_size: i64,
    pub batch_sleep: std::time::Duration,
}

impl Default for RetentionConfig {
    fn default() -> Self {
        Self {
            default_retention_days: DEFAULT_RETENTION_DAYS,
            service_log_retention_days: DEFAULT_SERVICE_LOG_RETENTION_DAYS,
            batch_size: DEFAULT_BATCH_SIZE,
            batch_sleep: std::time::Duration::from_millis(DEFAULT_BATCH_SLEEP_MS),
        }
    }
}

impl RetentionConfig {
    pub fn validate(&self) -> QueueResult<()> {
        validate_retention_days("default_retention_days", self.default_retention_days)?;
        validate_retention_days(
            "service_log_retention_days",
            self.service_log_retention_days,
        )?;
        validate_batch_size(self.batch_size)?;
        Ok(())
    }
}

#[derive(Debug)]
struct WorkspacePolicy {
    workspace_id: String,
    retention_days: i64,
}

#[derive(Debug)]
struct TeamPolicy {
    workspace_id: String,
    team_name: String,
    retention_days: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WorkspaceRetentionResult {
    pub workspace_id: String,
    pub run_log_deleted: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RetentionResult {
    pub workspaces: Vec<WorkspaceRetentionResult>,
    pub service_log_deleted: u64,
}

async fn load_workspace_policies(
    db: &PgPool,
    default_retention_days: i64,
) -> QueueResult<Vec<WorkspacePolicy>> {
    let rows = sqlx::query!(
        r#"
        SELECT w.id as "workspace_id!", ws.log_retention_days
        FROM workspace w
        LEFT JOIN workspace_settings ws ON ws.workspace_id = w.id
        "#
    )
    .fetch_all(db)
    .await?;

    rows.into_iter()
        .map(|row| {
            let retention_days = row
                .log_retention_days
                .map(|days| days as i64)
                .unwrap_or(default_retention_days);
            validate_retention_days("workspace log_retention_days", retention_days)?;
            Ok(WorkspacePolicy {
                workspace_id: row.workspace_id,
                retention_days,
            })
        })
        .collect()
}

async fn load_team_policies(db: &PgPool) -> QueueResult<Vec<TeamPolicy>> {
    let rows = sqlx::query!(
        r#"
        SELECT workspace_id, team_name, log_retention_days as "log_retention_days!"
        FROM team_quota
        WHERE log_retention_days IS NOT NULL
        "#
    )
    .fetch_all(db)
    .await?;

    rows.into_iter()
        .map(|row| {
            let retention_days = row.log_retention_days as i64;
            validate_retention_days("team log_retention_days", retention_days)?;
            Ok(TeamPolicy {
                workspace_id: row.workspace_id,
                team_name: row.team_name,
                retention_days,
            })
        })
        .collect()
}

fn validate_retention_days(field: &str, days: i64) -> QueueResult<()> {
    if !(0..=MAX_RETENTION_DAYS).contains(&days) {
        return Err(QueueError::Other(format!(
            "{field} must be between 0 and {MAX_RETENTION_DAYS} days"
        )));
    }
    Ok(())
}

fn validate_batch_size(batch_size: i64) -> QueueResult<()> {
    if !(1..=MAX_BATCH_SIZE).contains(&batch_size) {
        return Err(QueueError::Other(format!(
            "batch_size must be between 1 and {MAX_BATCH_SIZE}"
        )));
    }
    Ok(())
}

async fn cleanup_workspace_run_logs(
    db: &PgPool,
    workspace_id: &str,
    cutoff: chrono::DateTime<chrono::Utc>,
    exclude_teams: &[String],
    batch_size: i64,
    batch_sleep: std::time::Duration,
) -> QueueResult<u64> {
    let mut total: u64 = 0;
    loop {
        let result = sqlx::query!(
            "DELETE FROM run_log WHERE id IN (
                SELECT rl.id FROM run_log rl
                JOIN run r ON r.id = rl.run_id
                WHERE r.workspace_id = $1
                  AND rl.created_at < $2
                  AND (r.team_owner IS NULL OR r.team_owner != ALL($3))
                LIMIT $4
            )",
            workspace_id,
            cutoff,
            exclude_teams,
            batch_size,
        )
        .execute(db)
        .await?;

        let deleted = result.rows_affected();
        total += deleted;

        if deleted < batch_size as u64 {
            break;
        }
        tokio::time::sleep(batch_sleep).await;
    }
    Ok(total)
}

async fn cleanup_team_run_logs(
    db: &PgPool,
    workspace_id: &str,
    team_name: &str,
    cutoff: chrono::DateTime<chrono::Utc>,
    batch_size: i64,
    batch_sleep: std::time::Duration,
) -> QueueResult<u64> {
    let mut total: u64 = 0;
    loop {
        let result = sqlx::query!(
            "DELETE FROM run_log WHERE id IN (
                SELECT rl.id FROM run_log rl
                JOIN run r ON r.id = rl.run_id
                WHERE r.workspace_id = $1
                  AND r.team_owner = $2
                  AND rl.created_at < $3
                LIMIT $4
            )",
            workspace_id,
            team_name,
            cutoff,
            batch_size,
        )
        .execute(db)
        .await?;

        let deleted = result.rows_affected();
        total += deleted;

        if deleted < batch_size as u64 {
            break;
        }
        tokio::time::sleep(batch_sleep).await;
    }
    Ok(total)
}

async fn cleanup_service_logs(
    db: &PgPool,
    cutoff: chrono::DateTime<chrono::Utc>,
    batch_size: i64,
    batch_sleep: std::time::Duration,
) -> QueueResult<u64> {
    let mut total: u64 = 0;
    loop {
        let result = sqlx::query!(
            "DELETE FROM service_log WHERE id IN (
                SELECT id FROM service_log WHERE created_at < $1 LIMIT $2
            )",
            cutoff,
            batch_size,
        )
        .execute(db)
        .await?;

        let deleted = result.rows_affected();
        total += deleted;

        if deleted < batch_size as u64 {
            break;
        }
        tokio::time::sleep(batch_sleep).await;
    }
    Ok(total)
}

/// Execute log retention cleanup using per-workspace and per-team policies.
///
/// 1. Load workspace policies (from workspace_settings)
/// 2. Load team overrides (from team_quota)
/// 3. For each workspace: delete run_log rows older than workspace retention,
///    excluding teams with their own override
/// 4. For each team override: delete with team-specific cutoff
/// 5. Delete old service_log rows (global retention)
#[tracing::instrument(
    name = "retention::execute",
    skip(db, config),
    fields(
        default_retention_days = config.default_retention_days,
        service_log_retention_days = config.service_log_retention_days,
        batch_size = config.batch_size,
    )
)]
pub async fn execute_retention(
    db: &PgPool,
    config: &RetentionConfig,
) -> QueueResult<RetentionResult> {
    config.validate()?;

    let now = chrono::Utc::now();

    let ws_policies = load_workspace_policies(db, config.default_retention_days).await?;
    let team_policies = load_team_policies(db).await?;

    let mut workspaces = Vec::new();

    for ws in &ws_policies {
        let cutoff = now - chrono::Duration::days(ws.retention_days);

        // Collect team names that have overrides for this workspace
        let exclude_teams: Vec<String> = team_policies
            .iter()
            .filter(|t| t.workspace_id == ws.workspace_id)
            .map(|t| t.team_name.clone())
            .collect();

        let mut ws_deleted = cleanup_workspace_run_logs(
            db,
            &ws.workspace_id,
            cutoff,
            &exclude_teams,
            config.batch_size,
            config.batch_sleep,
        )
        .await?;

        // Apply team-specific overrides
        for team in team_policies
            .iter()
            .filter(|t| t.workspace_id == ws.workspace_id)
        {
            let team_cutoff = now - chrono::Duration::days(team.retention_days);
            let team_deleted = cleanup_team_run_logs(
                db,
                &ws.workspace_id,
                &team.team_name,
                team_cutoff,
                config.batch_size,
                config.batch_sleep,
            )
            .await?;
            ws_deleted += team_deleted;

            if team_deleted > 0 {
                tracing::info!(
                    workspace_id = %ws.workspace_id,
                    team = %team.team_name,
                    deleted = team_deleted,
                    retention_days = team.retention_days,
                    "team run_log cleanup"
                );
            }
        }

        if ws_deleted > 0 {
            tracing::info!(
                workspace_id = %ws.workspace_id,
                deleted = ws_deleted,
                retention_days = ws.retention_days,
                "workspace run_log cleanup"
            );
        }

        workspaces.push(WorkspaceRetentionResult {
            workspace_id: ws.workspace_id.clone(),
            run_log_deleted: ws_deleted,
        });
    }

    // Service log cleanup (global)
    let svc_cutoff = now - chrono::Duration::days(config.service_log_retention_days);
    let service_log_deleted =
        cleanup_service_logs(db, svc_cutoff, config.batch_size, config.batch_sleep).await?;

    if service_log_deleted > 0 {
        tracing::info!(
            deleted = service_log_deleted,
            retention_days = config.service_log_retention_days,
            "service_log cleanup"
        );
    }

    Ok(RetentionResult {
        workspaces,
        service_log_deleted,
    })
}

/// Submit a global retention run, recorded under `workspace_id` for audit.
pub async fn submit_retention_run(db: &PgPool, workspace_id: &str) -> QueueResult<Uuid> {
    submit_run(
        db,
        NewRun {
            workspace_id,
            kind: RunKind::Maintenance,
            script_hash: None,
            script_path: None,
            raw_code: None,
            language: None,
            args: None,
            flow_value: None,
            tag: "default",
            parent_run: None,
            root_run: None,
            flow_step_id: None,
            team_owner: None,
            created_by: "system",
            trace_id: None,
            span_id: None,
            scheduled_for: None,
            priority: Some(-100),
            cpus: Some(0.1),
            memory_mb: Some(64),
            disk_mb: Some(0),
            requirements: vec![],
            timeout: Some(3600),
            custom_image: None,
            schedule_id: None,
            scheduled_time: None,
            data_interval_end: None,
            trigger_id: None,
            trigger_context: None,
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_retention_config_default() {
        let config = RetentionConfig::default();
        assert_eq!(config.default_retention_days, 30);
        assert_eq!(config.service_log_retention_days, 14);
        assert_eq!(config.batch_size, 5000);
        assert_eq!(config.batch_sleep, std::time::Duration::from_millis(100));
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_retention_config_rejects_negative_retention_days() {
        let config = RetentionConfig {
            default_retention_days: -1,
            ..RetentionConfig::default()
        };

        assert!(config.validate().is_err());
    }

    #[test]
    fn test_retention_config_rejects_zero_batch_size() {
        let config = RetentionConfig {
            batch_size: 0,
            ..RetentionConfig::default()
        };

        assert!(config.validate().is_err());
    }

    #[test]
    fn test_retention_result_serialization() {
        let result = RetentionResult {
            workspaces: vec![
                WorkspaceRetentionResult {
                    workspace_id: "ws-1".to_string(),
                    run_log_deleted: 100,
                },
                WorkspaceRetentionResult {
                    workspace_id: "ws-2".to_string(),
                    run_log_deleted: 0,
                },
            ],
            service_log_deleted: 50,
        };

        let json = match serde_json::to_value(&result) {
            Ok(json) => json,
            Err(err) => panic!("retention result should serialize: {err}"),
        };
        assert_eq!(json["workspaces"][0]["workspace_id"], "ws-1");
        assert_eq!(json["workspaces"][0]["run_log_deleted"], 100);
        assert_eq!(json["service_log_deleted"], 50);
    }
}
