use crate::{NewRun, QueueResult, submit_run};
use coveflow_types::{RunKind, ScriptLang};
use sqlx::PgPool;
use uuid::Uuid;

/// Result of a rerun operation.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RerunResult {
    pub new_run_id: Uuid,
    pub original_run_id: Uuid,
}

/// Create a new run based on an existing run's parameters.
///
/// If `use_latest_version` is true and the original run had a `script_hash`,
/// the latest hash for that script_path will be looked up from the script table.
#[tracing::instrument(
    name = "queue::rerun",
    skip(db),
    fields(%original_run_id, %created_by, use_latest_version)
)]
pub async fn rerun(
    db: &PgPool,
    original_run_id: Uuid,
    created_by: &str,
    use_latest_version: bool,
) -> QueueResult<RerunResult> {
    // Fetch original run parameters
    let original = sqlx::query!(
        "SELECT workspace_id, kind, script_hash, script_path, raw_code, language,
                args, flow_value, tag, parent_run, root_run, flow_step_id,
                cpus, memory_mb, disk_mb, team_owner,
                requirements, timeout, custom_image
         FROM run WHERE id = $1",
        original_run_id
    )
    .fetch_optional(db)
    .await?
    .ok_or_else(|| crate::QueueError::Other(format!("run {original_run_id} not found")))?;

    // Optionally look up latest script version
    let script_hash = if use_latest_version {
        if let Some(path) = &original.script_path {
            let latest = sqlx::query_scalar!(
                "SELECT hash FROM script
                 WHERE workspace_id = $1 AND path = $2
                 ORDER BY created_at DESC LIMIT 1",
                original.workspace_id,
                path,
            )
            .fetch_optional(db)
            .await?;
            latest.or(original.script_hash.clone())
        } else {
            original.script_hash.clone()
        }
    } else {
        original.script_hash.clone()
    };

    let new_run = NewRun {
        workspace_id: &original.workspace_id,
        kind: original
            .kind
            .parse::<RunKind>()
            .map_err(crate::QueueError::Other)?,
        script_hash: script_hash.as_deref(),
        script_path: original.script_path.as_deref(),
        raw_code: original.raw_code.as_deref(),
        language: original
            .language
            .as_deref()
            .map(|l| l.parse::<ScriptLang>())
            .transpose()
            .map_err(crate::QueueError::Other)?,
        args: original.args.clone(),
        flow_value: original.flow_value.clone(),
        tag: &original.tag,
        parent_run: original.parent_run,
        root_run: original.root_run,
        flow_step_id: original.flow_step_id.as_deref(),
        team_owner: original.team_owner.as_deref(),
        created_by,
        trace_id: None,
        span_id: None,
        scheduled_for: None, // run immediately
        priority: None,
        cpus: Some(original.cpus as f32),
        memory_mb: Some(original.memory_mb),
        disk_mb: Some(original.disk_mb),
        requirements: original.requirements,
        timeout: original.timeout,
        custom_image: original.custom_image.as_deref(),
        schedule_id: None,
        scheduled_time: None,
        data_interval_end: None,
        trigger_id: None,
        trigger_context: None,
    };

    let new_run_id = submit_run(db, new_run).await?;

    // Set rerun_of link
    sqlx::query!(
        "UPDATE run SET rerun_of = $2 WHERE id = $1",
        new_run_id,
        original_run_id,
    )
    .execute(db)
    .await?;

    tracing::info!(
        new_run_id = %new_run_id,
        original_run_id = %original_run_id,
        use_latest_version,
        "rerun created"
    );

    Ok(RerunResult {
        new_run_id,
        original_run_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rerun_result_serialization() {
        let result = RerunResult {
            new_run_id: Uuid::nil(),
            original_run_id: Uuid::nil(),
        };
        let json = match serde_json::to_string(&result) {
            Ok(json) => json,
            Err(err) => panic!("rerun result should serialize: {err}"),
        };
        assert!(json.contains("new_run_id"));
        assert!(json.contains("original_run_id"));
    }
}
