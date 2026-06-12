use coveflow_queue::RetentionConfig;
use sqlx::PgPool;

use crate::error::{SandboxError, SandboxResult};

/// Execute a maintenance task based on the run's args.
///
/// Currently supports log retention cleanup. The run.args JSON can optionally
/// override default retention config values:
/// - `default_retention_days`: override global default (30)
/// - `service_log_retention_days`: override service log retention (14)
/// - `batch_size`: override batch size (5000)
#[tracing::instrument(name = "maintenance::exec", skip(db, run), fields(run_id = %run.id))]
pub async fn exec_maintenance(
    db: &PgPool,
    run: &coveflow_types::run::Run,
) -> SandboxResult<serde_json::Value> {
    let config = parse_retention_config(run.args.as_ref())?;

    tracing::info!(
        default_retention_days = config.default_retention_days,
        service_log_retention_days = config.service_log_retention_days,
        batch_size = config.batch_size,
        "starting log retention cleanup"
    );

    let result = coveflow_queue::execute_retention(db, &config)
        .await
        .map_err(|e| SandboxError::Other(format!("retention failed: {e}")))?;

    let total_run_log: u64 = result.workspaces.iter().map(|w| w.run_log_deleted).sum();
    tracing::info!(
        total_run_log_deleted = total_run_log,
        service_log_deleted = result.service_log_deleted,
        workspaces_processed = result.workspaces.len(),
        "log retention cleanup complete"
    );

    serde_json::to_value(&result).map_err(|e| SandboxError::Other(format!("serialize result: {e}")))
}

fn parse_retention_config(args: Option<&serde_json::Value>) -> SandboxResult<RetentionConfig> {
    let mut config = RetentionConfig::default();

    if let Some(args) = args {
        if let Some(d) = args.get("default_retention_days").and_then(|v| v.as_i64()) {
            config.default_retention_days = d;
        }
        if let Some(d) = args
            .get("service_log_retention_days")
            .and_then(|v| v.as_i64())
        {
            config.service_log_retention_days = d;
        }
        if let Some(b) = args.get("batch_size").and_then(|v| v.as_i64()) {
            config.batch_size = b;
        }
    }

    config
        .validate()
        .map_err(|e| SandboxError::Other(format!("invalid retention config: {e}")))?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_config(result: SandboxResult<RetentionConfig>) -> RetentionConfig {
        match result {
            Ok(config) => config,
            Err(err) => panic!("retention config should be valid: {err}"),
        }
    }

    #[test]
    fn test_parse_retention_config_defaults() {
        let config = valid_config(parse_retention_config(None));
        assert_eq!(config.default_retention_days, 30);
        assert_eq!(config.service_log_retention_days, 14);
        assert_eq!(config.batch_size, 5000);
    }

    #[test]
    fn test_parse_retention_config_overrides() {
        let args = serde_json::json!({
            "default_retention_days": 7,
            "service_log_retention_days": 3,
            "batch_size": 1000,
        });
        let config = valid_config(parse_retention_config(Some(&args)));
        assert_eq!(config.default_retention_days, 7);
        assert_eq!(config.service_log_retention_days, 3);
        assert_eq!(config.batch_size, 1000);
    }

    #[test]
    fn test_parse_retention_config_partial_overrides() {
        let args = serde_json::json!({
            "default_retention_days": 60,
        });
        let config = valid_config(parse_retention_config(Some(&args)));
        assert_eq!(config.default_retention_days, 60);
        assert_eq!(config.service_log_retention_days, 14); // unchanged
        assert_eq!(config.batch_size, 5000); // unchanged
    }

    #[test]
    fn test_parse_retention_config_rejects_negative_days() {
        let args = serde_json::json!({
            "default_retention_days": -1,
        });

        assert!(parse_retention_config(Some(&args)).is_err());
    }

    #[test]
    fn test_parse_retention_config_rejects_zero_batch_size() {
        let args = serde_json::json!({
            "batch_size": 0,
        });

        assert!(parse_retention_config(Some(&args)).is_err());
    }
}
