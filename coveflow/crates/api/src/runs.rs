use std::convert::Infallible;

use axum::Extension;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use futures::stream::Stream;
use sqlx::PgPool;
use uuid::Uuid;

use coveflow_queue::{NewRun, build_run_context};
use coveflow_types::run_context::RunContext;
use coveflow_types::scripts::{is_valid_runtime, normalize_requirements};
use coveflow_types::{RunKind, ScriptLang};

use crate::auth::AuthedUser;
use crate::common::{parse_and_verify_run, parse_level};
use crate::error::ApiError;

const DEFAULT_PAGE_LIMIT: i64 = 50;
const MAX_PAGE_LIMIT: i64 = 200;
const DEFAULT_WAIT_TIMEOUT_SECS: u64 = 600;
const MAX_WAIT_TIMEOUT_SECS: u64 = 3600;

struct ResolvedScript {
    hash: String,
    path: String,
    content: String,
    language: ScriptLang,
    runtime: Option<String>,
    requirements: Vec<String>,
}

/// Look up a saved script and snapshot its executable fields onto the run.
/// Prefers script_hash lookup; falls back to latest version at script_path.
/// Missing script references are rejected here so invalid runs do not reach the queue.
async fn resolve_script(
    db: &PgPool,
    workspace_id: &str,
    req: &CreateRunRequest,
) -> Result<ResolvedScript, ApiError> {
    if let Some(hash) = &req.script_hash {
        let script = sqlx::query_as!(
            ResolvedScript,
            r#"SELECT hash, path, content, language as "language: ScriptLang", runtime, requirements
               FROM script
               WHERE workspace_id = $1 AND hash = $2"#,
            workspace_id,
            hash
        )
        .fetch_optional(db)
        .await?
        .ok_or(ApiError::NotFound)?;
        return Ok(script);
    }

    if let Some(path) = &req.script_path {
        let script = sqlx::query_as!(
            ResolvedScript,
            r#"SELECT hash, path, content, language as "language: ScriptLang", runtime, requirements
               FROM script
               WHERE workspace_id = $1 AND path = $2
               ORDER BY created_at DESC
               LIMIT 1"#,
            workspace_id,
            path
        )
        .fetch_optional(db)
        .await?
        .ok_or(ApiError::NotFound)?;
        return Ok(script);
    }

    Err(ApiError::BadRequest(
        "script runs require script_hash or script_path".into(),
    ))
}

fn build_new_run<'a>(
    req: &'a CreateRunRequest,
    workspace_id: &'a str,
    email: &'a str,
) -> NewRun<'a> {
    let tag = req.tag.as_deref().unwrap_or("default");
    NewRun {
        workspace_id,
        kind: req.kind.clone(),
        script_hash: req.script_hash.as_deref(),
        script_path: req.script_path.as_deref(),
        raw_code: req.raw_code.as_deref(),
        language: req.language.clone(),
        args: req.args.clone(),
        flow_value: None,
        tag,
        parent_run: None,
        root_run: None,
        flow_step_id: None,
        team_owner: req.team_owner.as_deref(),
        created_by: email,
        trace_id: None,
        span_id: None,
        scheduled_for: req.scheduled_for,
        priority: req.priority,
        cpus: req.cpus,
        memory_mb: req.memory_mb,
        disk_mb: req.disk_mb,
        requirements: req.requirements.clone().unwrap_or_default(),
        timeout: req.timeout,
        custom_image: req.custom_image.as_deref(),
        schedule_id: None,
        scheduled_time: None,
        data_interval_end: None,
        trigger_id: None,
        trigger_context: None,
    }
}

fn validate_custom_image(custom_image: Option<&str>) -> Result<(), ApiError> {
    if !is_valid_runtime(custom_image) {
        return Err(ApiError::BadRequest(format!(
            "invalid custom_image: {:?}",
            custom_image
        )));
    }

    Ok(())
}

// Hard limits enforced at the API boundary. Mirror the popover's soft limits but
// allow a slightly wider envelope so users opting in via team quota can push limits
// without redeploying. Anything outside these is a programmer/abuse error.
//
// Note: `priority` overflow (> i16::MAX) is rejected at serde deserialization,
// so only the negative branch needs an explicit test in validate_resource_limits.
const TIMEOUT_MIN: i32 = 1;
const TIMEOUT_MAX: i32 = 86_400; // 24h
const CPUS_MIN: f32 = 0.1;
const CPUS_MAX: f32 = 64.0;
const MEMORY_MIN: i32 = 1;
const MEMORY_MAX: i32 = 1_048_576; // 1 TiB
const DISK_MIN: i32 = 1;
const DISK_MAX: i32 = 10_485_760; // 10 TiB
const PRIORITY_MIN: i16 = 0;
const PRIORITY_MAX: i16 = i16::MAX;

fn validate_resource_limits(req: &CreateRunRequest) -> Result<(), ApiError> {
    if let Some(t) = req.timeout
        && !(TIMEOUT_MIN..=TIMEOUT_MAX).contains(&t)
    {
        return Err(ApiError::BadRequest(format!(
            "timeout must be within {TIMEOUT_MIN}..={TIMEOUT_MAX} seconds (got {t})"
        )));
    }
    if let Some(c) = req.cpus
        && !(CPUS_MIN..=CPUS_MAX).contains(&c)
    {
        return Err(ApiError::BadRequest(format!(
            "cpus must be within {CPUS_MIN}..={CPUS_MAX} (got {c})"
        )));
    }
    if let Some(m) = req.memory_mb
        && !(MEMORY_MIN..=MEMORY_MAX).contains(&m)
    {
        return Err(ApiError::BadRequest(format!(
            "memory_mb must be within {MEMORY_MIN}..={MEMORY_MAX} (got {m})"
        )));
    }
    if let Some(d) = req.disk_mb
        && !(DISK_MIN..=DISK_MAX).contains(&d)
    {
        return Err(ApiError::BadRequest(format!(
            "disk_mb must be within {DISK_MIN}..={DISK_MAX} (got {d})"
        )));
    }
    if let Some(p) = req.priority
        && !(PRIORITY_MIN..=PRIORITY_MAX).contains(&p)
    {
        return Err(ApiError::BadRequest(format!(
            "priority must be within {PRIORITY_MIN}..={PRIORITY_MAX} (got {p})"
        )));
    }
    Ok(())
}

fn validate_run_options(
    _language: Option<&ScriptLang>,
    custom_image: Option<&str>,
    _requirements: &[String],
) -> Result<(), ApiError> {
    validate_custom_image(custom_image)?;
    Ok(())
}

fn validate_create_run_request(req: &CreateRunRequest, user: &AuthedUser) -> Result<(), ApiError> {
    match req.kind {
        RunKind::Script => {
            if req.script_hash.is_none() && req.script_path.is_none() {
                return Err(ApiError::BadRequest(
                    "script runs require script_hash or script_path".into(),
                ));
            }
            if req.raw_code.is_some() {
                return Err(ApiError::BadRequest(
                    "script runs must reference a saved script, not raw_code".into(),
                ));
            }
            if let Some(path) = &req.script_path {
                user.require_writer(path)?;
            }
        }
        RunKind::Preview => {
            if req.raw_code.is_none() {
                return Err(ApiError::BadRequest("preview runs require raw_code".into()));
            }
            if req.language.is_none() {
                return Err(ApiError::BadRequest("preview runs require language".into()));
            }
        }
        RunKind::Maintenance => {
            return Err(ApiError::BadRequest(
                "maintenance runs can only be submitted by the system".into(),
            ));
        }
        _ => {}
    }

    validate_resource_limits(req)?;

    let requirements = normalize_requirements(req.requirements.clone());
    validate_run_options(
        req.language.as_ref(),
        req.custom_image.as_deref(),
        &requirements,
    )
}

async fn prepare_create_run_request(
    db: &PgPool,
    workspace_id: &str,
    req: &mut CreateRunRequest,
    user: &AuthedUser,
) -> Result<(), ApiError> {
    validate_create_run_request(req, user)?;

    req.requirements = match normalize_requirements(req.requirements.take()) {
        requirements if requirements.is_empty() => None,
        requirements => Some(requirements),
    };

    if req.kind == RunKind::Script {
        let script = resolve_script(db, workspace_id, req).await?;
        user.require_writer(&script.path)?;

        if let Some(path) = &req.script_path
            && path != &script.path
        {
            return Err(ApiError::BadRequest(
                "script_hash and script_path refer to different scripts".into(),
            ));
        }

        req.script_hash = Some(script.hash);
        req.script_path = Some(script.path);
        req.raw_code = Some(script.content);
        req.language = Some(script.language);

        if req.requirements.is_none() && !script.requirements.is_empty() {
            req.requirements = Some(script.requirements);
        }
        if req.custom_image.is_none() {
            req.custom_image = script.runtime;
        }

        let requirements = req.requirements.clone().unwrap_or_default();
        validate_run_options(
            req.language.as_ref(),
            req.custom_image.as_deref(),
            &requirements,
        )?;
    }

    Ok(())
}

#[derive(serde::Deserialize)]
pub struct CreateRunRequest {
    pub kind: RunKind,
    pub script_hash: Option<String>,
    pub script_path: Option<String>,
    pub raw_code: Option<String>,
    pub language: Option<ScriptLang>,
    pub args: Option<serde_json::Value>,
    pub tag: Option<String>,
    pub scheduled_for: Option<DateTime<Utc>>,
    pub priority: Option<i16>,
    pub cpus: Option<f32>,
    pub memory_mb: Option<i32>,
    pub disk_mb: Option<i32>,
    pub requirements: Option<Vec<String>>,
    pub timeout: Option<i32>,
    pub custom_image: Option<String>,
    pub team_owner: Option<String>,
}

#[derive(serde::Serialize)]
pub struct RunCreated {
    pub id: String,
}

#[derive(serde::Serialize)]
pub struct RunResponse {
    pub id: String,
    pub workspace_id: String,
    pub kind: String,
    pub script_hash: Option<String>,
    pub script_path: Option<String>,
    pub raw_code: Option<String>,
    /// Flow execution progress (only for kind=flow/flow_preview).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flow_status: Option<serde_json::Value>,
    /// The flow's DAG definition (FlowSpec JSON) for kind=flow/flow_preview, so
    /// the run page can render the graph. Absent for non-flow runs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flow_value: Option<serde_json::Value>,
    pub language: Option<String>,
    pub args: Option<serde_json::Value>,
    pub tag: String,
    pub parent_run: Option<String>,
    pub root_run: Option<String>,
    pub requirements: Vec<String>,
    pub timeout: Option<i32>,
    pub cpus: f32,
    pub memory_mb: i32,
    pub disk_mb: i32,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
    pub status: String,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub duration_ms: Option<i32>,
    pub result: Option<serde_json::Value>,
    pub canceled_by: Option<String>,
    pub canceled_reason: Option<String>,
    pub marked_by: Option<String>,
    pub mark_reason: Option<String>,
    /// Airflow-style execution context (logical date, data interval, schedule
    /// meta, params, upstream step results). Same data the run's script receives
    /// as `ctx`; powers the run detail "Run context" panel.
    pub context: RunContext,
}

#[derive(serde::Deserialize)]
pub struct ListRunsQuery {
    pub status: Option<String>,
    /// Exact run kind filter (script / flow / preview / flow_preview / maintenance).
    pub kind: Option<String>,
    pub script_path: Option<String>,
    pub created_by: Option<String>,
    pub created_after_ms: Option<i64>,
    pub created_before_ms: Option<i64>,
    /// Only runs triggered by this schedule (powers the per-schedule history view).
    pub schedule_id: Option<Uuid>,
    /// Only runs fired by this trigger (powers the per-webhook history view).
    pub trigger_id: Option<Uuid>,
    /// Column to sort by (whitelisted; defaults to created_at) and direction.
    pub sort: Option<String>,
    pub order: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(sqlx::FromRow)]
struct RunListRow {
    id: Uuid,
    kind: String,
    script_path: Option<String>,
    tag: String,
    created_by: String,
    created_at: DateTime<Utc>,
    queue_running: Option<bool>,
    queue_started_at: Option<DateTime<Utc>>,
    suspended: bool,
    completed_success: Option<bool>,
    completed_canceled_by: Option<String>,
    completed_marked_by: Option<String>,
    completed_duration_ms: Option<i32>,
    completed_at: Option<DateTime<Utc>>,
    // Flow lineage: for a flow's child run, the top-level flow run id, that flow's
    // path, and which node produced this child.
    root_run: Option<Uuid>,
    flow_step_id: Option<String>,
    flow_path: Option<String>,
    scheduled_time: Option<DateTime<Utc>>,
}

#[derive(serde::Serialize)]
pub struct RunListItem {
    pub id: String,
    pub kind: String,
    pub script_path: Option<String>,
    pub tag: String,
    pub status: String,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub duration_ms: Option<i32>,
    pub success: Option<bool>,
    /// For a flow's child run: the top-level flow run id, its flow path, and the
    /// flow node that produced this run. All null for non-flow runs.
    pub flow_run_id: Option<String>,
    pub flow_path: Option<String>,
    pub flow_step_id: Option<String>,
    /// The cron occurrence ("logical date") a scheduled run is for; null otherwise.
    pub scheduled_time: Option<DateTime<Utc>>,
}

#[derive(serde::Deserialize)]
pub struct CancelRunRequest {
    pub reason: Option<String>,
    pub force: Option<bool>,
}

#[derive(serde::Deserialize)]
pub struct RerunRequest {
    pub use_latest_version: Option<bool>,
}

#[derive(serde::Deserialize)]
pub struct MarkRunRequest {
    pub reason: Option<String>,
    pub result: Option<serde_json::Value>,
}

#[derive(serde::Serialize)]
pub struct RunCompletedInfo {
    pub success: bool,
    pub result: Option<serde_json::Value>,
}

#[derive(serde::Serialize)]
pub struct RunLogsResponse {
    pub run_id: String,
    pub chunks: Vec<coveflow_queue::RunLogChunkRow>,
    pub next_cursor: Option<i64>,
    /// "queued" | "running" | "success" | "failure" | "cancelled"
    pub status: String,
    /// Present iff the run is in `run_completed`. Used by polling clients
    /// to know when to stop and what payload to surface.
    pub completed: Option<RunCompletedInfo>,
}

#[derive(serde::Deserialize)]
pub struct RunLogsQuery {
    pub level: Option<String>,
    pub after_chunk: Option<i64>,
    pub limit: Option<i64>,
}

#[derive(serde::Deserialize)]
pub struct RunWaitResultQuery {
    pub timeout: Option<u64>,
    pub queue_limit: Option<i64>,
}

fn derive_status(
    has_completed: bool,
    completed_success: Option<bool>,
    completed_canceled_by: Option<&str>,
    completed_marked_by: Option<&str>,
    queue_running: Option<bool>,
    // A flow that has dispatched children parks its queue row at
    // scheduled_for='infinity' (running=false). It is in-flight, not queued.
    suspended: bool,
) -> &'static str {
    if has_completed {
        // Mark overrides cancel: an admin override declares the run's final
        // verdict, regardless of any earlier cancel request.
        if completed_marked_by.is_some() {
            if completed_success == Some(true) {
                "success"
            } else {
                "failure"
            }
        } else if completed_canceled_by.is_some() {
            "cancelled"
        } else if completed_success == Some(true) {
            "success"
        } else {
            "failure"
        }
    } else if queue_running == Some(true) || suspended {
        "running"
    } else {
        "queued"
    }
}

#[tracing::instrument(name = "api::create_run", skip(db, user, req), fields(%workspace_id))]
pub async fn create_run(
    State(db): State<PgPool>,
    Extension(user): Extension<AuthedUser>,
    Path(workspace_id): Path<String>,
    Json(mut req): Json<CreateRunRequest>,
) -> Result<Response, ApiError> {
    prepare_create_run_request(&db, &workspace_id, &mut req, &user).await?;

    let new_run = build_new_run(&req, &workspace_id, &user.email);
    let run_id = coveflow_queue::submit_run(&db, new_run).await?;

    Ok((
        StatusCode::CREATED,
        Json(RunCreated {
            id: run_id.to_string(),
        }),
    )
        .into_response())
}

#[tracing::instrument(name = "api::list_runs", skip(db, _user, query), fields(%workspace_id))]
pub async fn list_runs(
    State(db): State<PgPool>,
    Extension(_user): Extension<AuthedUser>,
    Path(workspace_id): Path<String>,
    Query(query): Query<ListRunsQuery>,
) -> Result<Json<Vec<RunListItem>>, ApiError> {
    let limit = query
        .limit
        .unwrap_or(DEFAULT_PAGE_LIMIT)
        .min(MAX_PAGE_LIMIT);
    let offset = query.offset.unwrap_or(0);

    // Build the status filter as a SQL condition that mirrors derive_status().
    // status is computed from run_completed + run_queue joins, so we translate
    // each status value into the equivalent WHERE predicate.
    let status_sql = match query.status.as_deref() {
        Some("queued") => {
            "AND rc.id IS NULL AND (rq.running IS NULL OR rq.running = false) AND (rq.scheduled_for IS NULL OR rq.scheduled_for <> 'infinity')"
        }
        Some("running") => {
            "AND rc.id IS NULL AND (rq.running = true OR rq.scheduled_for = 'infinity')"
        }
        Some("success") => "AND rc.id IS NOT NULL AND rc.canceled_by IS NULL AND rc.success = true",
        Some("failure") => {
            "AND rc.id IS NOT NULL AND rc.canceled_by IS NULL AND rc.success = false"
        }
        Some("cancelled") => "AND rc.id IS NOT NULL AND rc.canceled_by IS NOT NULL",
        _ => "",
    };

    let script_path_pattern = query
        .script_path
        .as_ref()
        .map(|sp| format!("%{}%", sp.replace('%', "\\%").replace('_', "\\_")));
    let created_by_pattern = query
        .created_by
        .as_ref()
        .map(|cb| format!("%{}%", cb.replace('%', "\\%").replace('_', "\\_")));
    let created_after = query
        .created_after_ms
        .and_then(chrono::DateTime::from_timestamp_millis);
    let created_before = query
        .created_before_ms
        .and_then(chrono::DateTime::from_timestamp_millis);

    // Whitelist the sort column (interpolated into SQL, so never use the raw
    // value). `r.id` tiebreaks for stable pagination.
    let sort_col = match query.sort.as_deref() {
        Some("kind") => "r.kind",
        Some("script_path") => "r.script_path",
        Some("created_by") => "r.created_by",
        Some("duration_ms") => "rc.duration_ms",
        Some("scheduled_time") => "COALESCE(r.scheduled_time, root.scheduled_time)",
        _ => "r.created_at",
    };
    let sort_dir = if query.order.as_deref() == Some("asc") {
        "ASC"
    } else {
        "DESC"
    };

    let sql = format!(
        r#"
        SELECT
            r.id,
            r.kind,
            r.script_path,
            r.tag,
            r.created_by,
            r.created_at,
            rq.running        as queue_running,
            rq.started_at     as queue_started_at,
            COALESCE(rq.scheduled_for = 'infinity', false) as suspended,
            rc.success        as completed_success,
            rc.canceled_by    as completed_canceled_by,
            rc.marked_by      as completed_marked_by,
            rc.duration_ms    as completed_duration_ms,
            rc.completed_at   as completed_at,
            r.root_run        as root_run,
            r.flow_step_id    as flow_step_id,
            root.script_path  as flow_path,
            -- A scheduled flow's child node runs inherit the parent flow run's
            -- slot (logical date), Airflow-style, so the whole run tree agrees.
            COALESCE(r.scheduled_time, root.scheduled_time) as scheduled_time
        FROM run r
        LEFT JOIN run_queue rq ON rq.id = r.id
        LEFT JOIN run_completed rc ON rc.id = r.id
        -- The top-level flow run this child belongs to (skip self-referential roots).
        LEFT JOIN run root ON root.id = r.root_run AND root.id <> r.id
        WHERE r.workspace_id = $1
          -- Match the run's own path, or its parent flow's path so searching a
          -- flow surfaces the flow run plus all of its child node runs.
          AND ($4::text IS NULL OR r.script_path ILIKE $4 OR root.script_path ILIKE $4)
          AND ($5::text IS NULL OR r.created_by ILIKE $5)
          AND ($6::timestamptz IS NULL OR r.created_at >= $6)
          AND ($7::timestamptz IS NULL OR r.created_at <= $7)
          AND ($8::uuid IS NULL OR r.schedule_id = $8)
          AND ($9::text IS NULL OR r.kind = $9)
          AND ($10::uuid IS NULL OR r.trigger_id = $10)
          {status_sql}
        ORDER BY {sort_col} {sort_dir} NULLS LAST, r.created_at DESC, r.id DESC
        LIMIT $2 OFFSET $3
        "#
    );

    let rows = sqlx::query_as::<_, RunListRow>(&sql)
        .bind(&workspace_id)
        .bind(limit)
        .bind(offset)
        .bind(&script_path_pattern)
        .bind(&created_by_pattern)
        .bind(created_after)
        .bind(created_before)
        .bind(query.schedule_id)
        .bind(&query.kind)
        .bind(query.trigger_id)
        .fetch_all(&db)
        .await?;

    let items: Vec<RunListItem> = rows
        .into_iter()
        .map(|row| {
            let status = derive_status(
                row.completed_at.is_some(),
                row.completed_success,
                row.completed_canceled_by.as_deref(),
                row.completed_marked_by.as_deref(),
                row.queue_running,
                row.suspended,
            )
            .to_string();
            let duration_ms = effective_duration_ms(
                &row.kind,
                row.created_at,
                row.completed_at,
                row.completed_duration_ms,
            );
            RunListItem {
                id: row.id.to_string(),
                kind: row.kind,
                script_path: row.script_path,
                tag: row.tag,
                status,
                created_by: row.created_by,
                created_at: row.created_at,
                started_at: row.queue_started_at,
                completed_at: row.completed_at,
                duration_ms,
                success: row.completed_success,
                flow_run_id: row.root_run.map(|u| u.to_string()),
                flow_path: row.flow_path,
                flow_step_id: row.flow_step_id,
                scheduled_time: row.scheduled_time,
            }
        })
        .collect();

    Ok(Json(items))
}

/// Flow runs are re-entrant: their stored `duration_ms` is only the final
/// `advance_flow` pass, not the whole flow. Report wall-clock (completed -
/// created) for flow kinds; other kinds use the stored duration as-is.
fn effective_duration_ms(
    kind: &str,
    created_at: DateTime<Utc>,
    completed_at: Option<DateTime<Utc>>,
    stored: Option<i32>,
) -> Option<i32> {
    if matches!(kind, "flow" | "flow_preview") {
        // Clamp into i32 range: a long-running flow's ms can exceed i32::MAX
        // (~24.8 days), which would overflow/wrap on the cast.
        completed_at.map(|c| {
            (c - created_at)
                .num_milliseconds()
                .clamp(0, i32::MAX as i64) as i32
        })
    } else {
        stored
    }
}

#[tracing::instrument(name = "api::get_run", skip(db, _user), fields(%workspace_id, %run_id))]
pub async fn get_run(
    State(db): State<PgPool>,
    Extension(_user): Extension<AuthedUser>,
    Path((workspace_id, run_id)): Path<(String, String)>,
) -> Result<Json<RunResponse>, ApiError> {
    let run_id: Uuid = run_id
        .parse()
        .map_err(|_| ApiError::BadRequest("invalid run_id".into()))?;

    let row = sqlx::query!(
        r#"
        SELECT
            r.id,
            r.workspace_id,
            r.kind,
            r.script_hash,
            r.script_path,
            r.raw_code,
            r.language,
            r.args,
            r.tag,
            r.parent_run,
            r.root_run,
            r.requirements,
            r.timeout,
            r.cpus,
            r.memory_mb,
            r.disk_mb,
            r.created_by,
            r.created_at as "created_at!",
            rq.running     as "queue_running?",
            rq.started_at  as "queue_started_at",
            rc.success     as "completed_success?",
            rc.canceled_by as "completed_canceled_by",
            rc.canceled_reason as "completed_canceled_reason",
            rc.marked_by   as "completed_marked_by",
            rc.mark_reason as "completed_mark_reason",
            rc.duration_ms as "completed_duration_ms?",
            rc.completed_at as "completed_at?",
            rc.result       as "completed_result",
            r.flow_value    as "flow_value?",
            fs.flow_status  as "flow_status?",
            COALESCE(rq.scheduled_for = 'infinity', false) as "suspended!"
        FROM run r
        LEFT JOIN run_queue rq ON rq.id = r.id
        LEFT JOIN run_completed rc ON rc.id = r.id
        LEFT JOIN run_flow_status fs ON fs.run_id = r.id
        WHERE r.workspace_id = $1 AND r.id = $2
        "#,
        workspace_id,
        run_id,
    )
    .fetch_optional(&db)
    .await?
    .ok_or(ApiError::NotFound)?;

    let status = derive_status(
        row.completed_at.is_some(),
        row.completed_success,
        row.completed_canceled_by.as_deref(),
        row.completed_marked_by.as_deref(),
        row.queue_running,
        row.suspended,
    )
    .to_string();

    let duration_ms = effective_duration_ms(
        &row.kind,
        row.created_at,
        row.completed_at,
        row.completed_duration_ms,
    );

    let context = build_run_context(&db, run_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(RunResponse {
        id: row.id.to_string(),
        workspace_id: row.workspace_id,
        kind: row.kind,
        script_hash: row.script_hash,
        script_path: row.script_path,
        raw_code: row.raw_code,
        flow_status: row.flow_status,
        flow_value: row.flow_value,
        language: row.language,
        args: row.args,
        tag: row.tag,
        parent_run: row.parent_run.map(|u| u.to_string()),
        root_run: row.root_run.map(|u| u.to_string()),
        requirements: row.requirements,
        timeout: row.timeout,
        cpus: row.cpus,
        memory_mb: row.memory_mb,
        disk_mb: row.disk_mb,
        created_by: row.created_by,
        created_at: row.created_at,
        status,
        started_at: row.queue_started_at,
        completed_at: row.completed_at,
        duration_ms,
        result: row.completed_result,
        canceled_by: row.completed_canceled_by,
        canceled_reason: row.completed_canceled_reason,
        marked_by: row.completed_marked_by,
        mark_reason: row.completed_mark_reason,
        context,
    }))
}

#[tracing::instrument(name = "api::get_run_logs", skip(db, _user, query), fields(%workspace_id, %run_id))]
pub async fn get_run_logs(
    State(db): State<PgPool>,
    Extension(_user): Extension<AuthedUser>,
    Path((workspace_id, run_id)): Path<(String, String)>,
    Query(query): Query<RunLogsQuery>,
) -> Result<Json<RunLogsResponse>, ApiError> {
    let run_id = parse_and_verify_run(&db, &workspace_id, &run_id).await?;

    let after_id = query.after_chunk.unwrap_or(0);
    let limit = query
        .limit
        .unwrap_or(DEFAULT_PAGE_LIMIT)
        .min(MAX_PAGE_LIMIT);
    let min_level = query.level.as_deref().and_then(parse_level);

    let chunks =
        coveflow_queue::get_run_log_chunks(&db, run_id, after_id, min_level, limit).await?;

    let next_cursor = chunks.last().map(|c| c.id);

    let state = sqlx::query!(
        r#"
        SELECT
            rq.running      as "queue_running?",
            COALESCE(rq.scheduled_for = 'infinity', false) as "suspended!",
            rc.success      as "completed_success?",
            rc.canceled_by  as "completed_canceled_by",
            rc.marked_by    as "completed_marked_by",
            rc.result       as "completed_result"
        FROM run r
        LEFT JOIN run_queue rq ON rq.id = r.id
        LEFT JOIN run_completed rc ON rc.id = r.id
        WHERE r.id = $1
        "#,
        run_id,
    )
    .fetch_one(&db)
    .await?;

    let has_completed = state.completed_success.is_some();
    let status = derive_status(
        has_completed,
        state.completed_success,
        state.completed_canceled_by.as_deref(),
        state.completed_marked_by.as_deref(),
        state.queue_running,
        state.suspended,
    )
    .to_string();

    let completed = state.completed_success.map(|success| RunCompletedInfo {
        success,
        result: state.completed_result,
    });

    Ok(Json(RunLogsResponse {
        run_id: run_id.to_string(),
        chunks,
        next_cursor,
        status,
        completed,
    }))
}

#[tracing::instrument(name = "api::stream_run_logs", skip(db, _user, query), fields(%workspace_id, %run_id))]
pub async fn stream_run_logs(
    State(db): State<PgPool>,
    Extension(_user): Extension<AuthedUser>,
    Path((workspace_id, run_id)): Path<(String, String)>,
    Query(query): Query<RunLogsQuery>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let run_id = parse_and_verify_run(&db, &workspace_id, &run_id).await?;

    let min_level = query.level.as_deref().and_then(parse_level);
    let mut last_chunk_id = query.after_chunk.unwrap_or(0);
    let poll_interval = tokio::time::Duration::from_millis(500);

    let stream = async_stream::stream! {
        loop {
            // Fetch any new chunks
            match coveflow_queue::get_run_log_chunks(&db, run_id, last_chunk_id, min_level, 50).await {
                Ok(chunks) => {
                    for chunk in &chunks {
                        last_chunk_id = chunk.id;
                        let data = serde_json::json!({
                            "chunk_id": chunk.id,
                            "seq": chunk.seq,
                            "entries": chunk.entries,
                        });
                        let event = Event::default()
                            .event("log")
                            .data(data.to_string());
                        yield Ok(event);
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "SSE: failed to fetch log chunks");
                }
            }

            // Check if run is completed
            let completed = sqlx::query!(
                r#"SELECT success as "success!", result as "result"
                   FROM run_completed WHERE id = $1"#,
                run_id,
            )
            .fetch_optional(&db)
            .await;

            if let Ok(Some(row)) = completed {
                let event = Event::default()
                    .event("result")
                    .data(serde_json::json!({
                        "success": row.success,
                        "result": row.result,
                    }).to_string());
                yield Ok(event);
                return;
            }

            tokio::time::sleep(poll_interval).await;
        }
    };

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

/// WaitResultGuard: cancels the run on drop if not marked as done.
/// This ensures that if the HTTP connection is dropped, the run is canceled.
struct WaitResultGuard {
    done: bool,
    run_id: Uuid,
    db: PgPool,
    email: String,
}

impl Drop for WaitResultGuard {
    fn drop(&mut self) {
        if !self.done {
            let run_id = self.run_id;
            let db = self.db.clone();
            let email = self.email.clone();
            tracing::info!(run_id = %run_id, "HTTP connection broke, canceling run");
            tokio::spawn(async move {
                let _ = coveflow_queue::cancel_run(
                    &db,
                    run_id,
                    &email,
                    Some("HTTP connection disconnected"),
                    false,
                )
                .await;
            });
        }
    }
}

#[tracing::instrument(name = "api::run_wait_result", skip(db, user, wait_query, req), fields(%workspace_id))]
pub async fn run_wait_result(
    State(db): State<PgPool>,
    Extension(user): Extension<AuthedUser>,
    Path(workspace_id): Path<String>,
    Query(wait_query): Query<RunWaitResultQuery>,
    Json(mut req): Json<CreateRunRequest>,
) -> Result<Response, ApiError> {
    // Check queue limit if specified
    if let Some(queue_limit) = wait_query.queue_limit {
        let queue_count = sqlx::query_scalar!(r#"SELECT COUNT(*) as "count!" FROM run_queue"#,)
            .fetch_one(&db)
            .await?;

        if queue_count >= queue_limit {
            return Err(ApiError::ServiceUnavailable(format!(
                "queue limit exceeded: {queue_count}/{queue_limit}"
            )));
        }
    }

    prepare_create_run_request(&db, &workspace_id, &mut req, &user).await?;

    let new_run = build_new_run(&req, &workspace_id, &user.email);
    let run_id = coveflow_queue::submit_run(&db, new_run).await?;

    let mut guard = WaitResultGuard {
        done: false,
        run_id,
        db: db.clone(),
        email: user.email.clone(),
    };

    let timeout_secs = wait_query
        .timeout
        .unwrap_or(DEFAULT_WAIT_TIMEOUT_SECS)
        .min(MAX_WAIT_TIMEOUT_SECS);
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(timeout_secs);
    let poll_interval = tokio::time::Duration::from_millis(200);

    loop {
        // Check if run is completed
        let completed = sqlx::query!(
            r#"SELECT success as "success!", result as "result", duration_ms as "duration_ms!"
               FROM run_completed WHERE id = $1"#,
            run_id,
        )
        .fetch_optional(&db)
        .await?;

        if let Some(row) = completed {
            guard.done = true;

            // Fetch full run info for RunResponse
            let run_row = sqlx::query!(
                r#"
                SELECT
                    r.id, r.workspace_id, r.kind, r.script_hash, r.script_path,
                    r.raw_code, r.language, r.args, r.tag, r.parent_run, r.root_run,
                    r.requirements, r.timeout, r.cpus, r.memory_mb, r.disk_mb,
                    r.created_by, r.created_at as "created_at!",
                    rc.completed_at as "completed_at!",
                    rc.success as "success!",
                    rc.result as "result",
                    rc.duration_ms as "duration_ms!",
                    rc.canceled_by,
                    rc.canceled_reason,
                    rc.marked_by,
                    rc.mark_reason
                FROM run r
                JOIN run_completed rc ON rc.id = r.id
                WHERE r.id = $1
                "#,
                run_id,
            )
            .fetch_one(&db)
            .await?;

            let status = derive_status(
                true,
                Some(run_row.success),
                run_row.canceled_by.as_deref(),
                run_row.marked_by.as_deref(),
                None,
                false,
            )
            .to_string();

            let context = build_run_context(&db, run_id)
                .await
                .map_err(|e| ApiError::Internal(e.to_string()))?;

            return Ok(Json(RunResponse {
                id: run_row.id.to_string(),
                workspace_id: run_row.workspace_id,
                kind: run_row.kind,
                script_hash: run_row.script_hash,
                script_path: run_row.script_path,
                raw_code: run_row.raw_code,
                flow_status: None,
                flow_value: None,
                language: run_row.language,
                args: run_row.args,
                tag: run_row.tag,
                parent_run: run_row.parent_run.map(|u| u.to_string()),
                root_run: run_row.root_run.map(|u| u.to_string()),
                requirements: run_row.requirements,
                timeout: run_row.timeout,
                cpus: run_row.cpus,
                memory_mb: run_row.memory_mb,
                disk_mb: run_row.disk_mb,
                created_by: run_row.created_by,
                created_at: run_row.created_at,
                status,
                started_at: None,
                completed_at: Some(run_row.completed_at),
                duration_ms: Some(row.duration_ms),
                result: row.result,
                canceled_by: run_row.canceled_by,
                canceled_reason: run_row.canceled_reason,
                marked_by: run_row.marked_by,
                mark_reason: run_row.mark_reason,
                context,
            })
            .into_response());
        }

        // Check timeout
        if tokio::time::Instant::now() >= deadline {
            guard.done = true;
            return Err(ApiError::Timeout);
        }

        tokio::time::sleep(poll_interval).await;
    }
}

#[tracing::instrument(name = "api::cancel_run", skip(db, user, req), fields(%workspace_id, %run_id))]
pub async fn cancel_run_handler(
    State(db): State<PgPool>,
    Extension(user): Extension<AuthedUser>,
    Path((workspace_id, run_id)): Path<(String, String)>,
    Json(req): Json<CancelRunRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let run_id: Uuid = run_id
        .parse()
        .map_err(|_| ApiError::BadRequest("invalid run_id".into()))?;

    // Check run exists and belongs to workspace
    let run = sqlx::query!(
        "SELECT created_by FROM run WHERE id = $1 AND workspace_id = $2",
        run_id,
        workspace_id,
    )
    .fetch_optional(&db)
    .await?
    .ok_or(ApiError::NotFound)?;

    // Non-admin can only cancel own runs
    if !user.is_admin() && run.created_by != user.email {
        return Err(ApiError::Forbidden("can only cancel your own runs".into()));
    }

    // Force cancel is admin-only
    let force = req.force.unwrap_or(false);
    if force && !user.is_admin() {
        return Err(ApiError::Forbidden("force cancel requires admin".into()));
    }

    let outcome =
        coveflow_queue::cancel_run(&db, run_id, &user.email, req.reason.as_deref(), force).await?;

    Ok(Json(serde_json::json!({ "outcome": outcome })))
}

#[tracing::instrument(name = "api::rerun", skip(db, user, req), fields(%workspace_id, %run_id))]
pub async fn rerun_handler(
    State(db): State<PgPool>,
    Extension(user): Extension<AuthedUser>,
    Path((workspace_id, run_id)): Path<(String, String)>,
    Json(req): Json<RerunRequest>,
) -> Result<Response, ApiError> {
    let run_id: Uuid = run_id
        .parse()
        .map_err(|_| ApiError::BadRequest("invalid run_id".into()))?;

    // Verify run belongs to workspace
    let exists = sqlx::query_scalar!(
        r#"SELECT EXISTS(SELECT 1 FROM run WHERE id = $1 AND workspace_id = $2) as "exists!""#,
        run_id,
        workspace_id,
    )
    .fetch_one(&db)
    .await?;

    if !exists {
        return Err(ApiError::NotFound);
    }

    let use_latest = req.use_latest_version.unwrap_or(false);
    let result = coveflow_queue::rerun(&db, run_id, &user.email, use_latest).await?;

    Ok((
        StatusCode::CREATED,
        Json(RunCreated {
            id: result.new_run_id.to_string(),
        }),
    )
        .into_response())
}

#[tracing::instrument(name = "api::mark_success", skip(db, user, req), fields(%workspace_id, %run_id))]
pub async fn mark_success_handler(
    State(db): State<PgPool>,
    Extension(user): Extension<AuthedUser>,
    Path((workspace_id, run_id)): Path<(String, String)>,
    Json(req): Json<MarkRunRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin only".into()));
    }

    let run_id: Uuid = run_id
        .parse()
        .map_err(|_| ApiError::BadRequest("invalid run_id".into()))?;

    // Verify run belongs to workspace
    let exists = sqlx::query_scalar!(
        r#"SELECT EXISTS(SELECT 1 FROM run WHERE id = $1 AND workspace_id = $2) as "exists!""#,
        run_id,
        workspace_id,
    )
    .fetch_one(&db)
    .await?;

    if !exists {
        return Err(ApiError::NotFound);
    }

    coveflow_queue::mark_success(&db, run_id, &user.email, req.reason.as_deref(), req.result)
        .await?;

    Ok(Json(serde_json::json!({ "status": "ok" })))
}

#[tracing::instrument(name = "api::mark_fail", skip(db, user, req), fields(%workspace_id, %run_id))]
pub async fn mark_fail_handler(
    State(db): State<PgPool>,
    Extension(user): Extension<AuthedUser>,
    Path((workspace_id, run_id)): Path<(String, String)>,
    Json(req): Json<MarkRunRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin only".into()));
    }

    let run_id: Uuid = run_id
        .parse()
        .map_err(|_| ApiError::BadRequest("invalid run_id".into()))?;

    // Verify run belongs to workspace
    let exists = sqlx::query_scalar!(
        r#"SELECT EXISTS(SELECT 1 FROM run WHERE id = $1 AND workspace_id = $2) as "exists!""#,
        run_id,
        workspace_id,
    )
    .fetch_one(&db)
    .await?;

    if !exists {
        return Err(ApiError::NotFound);
    }

    coveflow_queue::mark_fail(&db, run_id, &user.email, req.reason.as_deref(), req.result).await?;

    Ok(Json(serde_json::json!({ "status": "ok" })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::WorkspaceRole;
    use std::collections::HashMap;

    fn test_user() -> AuthedUser {
        AuthedUser {
            email: "alice@example.com".to_string(),
            workspace_id: "ws-1".to_string(),
            role: WorkspaceRole::Admin,
            teams: Vec::new(),
            team_roles: HashMap::new(),
        }
    }

    fn create_run_request(kind: RunKind) -> CreateRunRequest {
        CreateRunRequest {
            kind,
            script_hash: None,
            script_path: None,
            raw_code: None,
            language: None,
            args: None,
            tag: None,
            scheduled_for: None,
            priority: None,
            cpus: None,
            memory_mb: None,
            disk_mb: None,
            requirements: None,
            timeout: None,
            custom_image: None,
            team_owner: None,
        }
    }

    #[test]
    fn test_derive_status_queued() {
        let status = derive_status(false, None, None, None, None, false);
        assert_eq!(status, "queued");
    }

    #[test]
    fn test_derive_status_running() {
        let status = derive_status(false, None, None, None, Some(true), false);
        assert_eq!(status, "running");
    }

    #[test]
    fn test_derive_status_suspended_flow_is_running() {
        // A suspended flow has running=false but is in-flight (waiting on children).
        let status = derive_status(false, None, None, None, Some(false), true);
        assert_eq!(status, "running");
    }

    #[test]
    fn test_derive_status_success() {
        let status = derive_status(true, Some(true), None, None, None, false);
        assert_eq!(status, "success");
    }

    #[test]
    fn test_derive_status_failure() {
        let status = derive_status(true, Some(false), None, None, None, false);
        assert_eq!(status, "failure");
    }

    #[test]
    fn test_derive_status_cancelled() {
        let status = derive_status(true, Some(false), Some("admin"), None, None, false);
        assert_eq!(status, "cancelled");
    }

    #[test]
    fn test_derive_status_mark_overrides_cancel_to_success() {
        // Run was cancelled, then admin marked it success → status should
        // reflect the mark override.
        let status = derive_status(true, Some(true), Some("alice"), Some("admin"), None, false);
        assert_eq!(status, "success");
    }

    #[test]
    fn test_derive_status_mark_overrides_cancel_to_failure() {
        let status = derive_status(true, Some(false), Some("alice"), Some("admin"), None, false);
        assert_eq!(status, "failure");
    }

    #[test]
    fn test_validate_create_run_rejects_maintenance() {
        let req = create_run_request(RunKind::Maintenance);
        let err = match validate_create_run_request(&req, &test_user()) {
            Ok(()) => panic!("maintenance runs should not be public API input"),
            Err(err) => err,
        };

        assert!(matches!(err, ApiError::BadRequest(message) if message.contains("maintenance")));
    }

    #[test]
    fn test_validate_custom_image_accepts_none_and_allowlisted_runtime() {
        assert!(validate_custom_image(None).is_ok());
        assert!(validate_custom_image(Some("python:3.11")).is_ok());
    }

    #[test]
    fn test_validate_custom_image_rejects_unknown_runtime() {
        let err = match validate_custom_image(Some("python:9.99")) {
            Ok(()) => panic!("unknown custom_image should be rejected"),
            Err(err) => err,
        };

        assert!(
            matches!(err, ApiError::BadRequest(message) if message.contains("invalid custom_image"))
        );
    }

    #[test]
    fn test_validate_run_options_accepts_python_options() {
        let requirements = vec!["requests".to_string()];

        assert!(
            validate_run_options(
                Some(&ScriptLang::Python3),
                Some("python:3.12"),
                &requirements,
            )
            .is_ok()
        );
    }

    #[test]
    fn test_validate_resource_limits_accepts_unset_fields() {
        let req = create_run_request(RunKind::Preview);
        assert!(validate_resource_limits(&req).is_ok());
    }

    #[test]
    fn test_validate_resource_limits_accepts_in_range_values() {
        let mut req = create_run_request(RunKind::Preview);
        req.timeout = Some(600);
        req.cpus = Some(1.5);
        req.memory_mb = Some(512);
        req.disk_mb = Some(1024);
        req.priority = Some(10);
        assert!(validate_resource_limits(&req).is_ok());
    }

    #[test]
    fn test_validate_resource_limits_rejects_negative_timeout() {
        let mut req = create_run_request(RunKind::Preview);
        req.timeout = Some(0);
        assert!(matches!(
            validate_resource_limits(&req),
            Err(ApiError::BadRequest(m)) if m.contains("timeout")
        ));
    }

    #[test]
    fn test_validate_resource_limits_rejects_excessive_timeout() {
        let mut req = create_run_request(RunKind::Preview);
        req.timeout = Some(100_000);
        assert!(matches!(
            validate_resource_limits(&req),
            Err(ApiError::BadRequest(m)) if m.contains("timeout")
        ));
    }

    #[test]
    fn test_validate_resource_limits_rejects_negative_cpus() {
        let mut req = create_run_request(RunKind::Preview);
        req.cpus = Some(0.0);
        assert!(matches!(
            validate_resource_limits(&req),
            Err(ApiError::BadRequest(m)) if m.contains("cpus")
        ));
    }

    #[test]
    fn test_validate_resource_limits_rejects_excessive_cpus() {
        let mut req = create_run_request(RunKind::Preview);
        req.cpus = Some(CPUS_MAX + 0.1);
        assert!(matches!(
            validate_resource_limits(&req),
            Err(ApiError::BadRequest(m)) if m.contains("cpus")
        ));
    }

    #[test]
    fn test_validate_resource_limits_rejects_negative_memory() {
        let mut req = create_run_request(RunKind::Preview);
        req.memory_mb = Some(0);
        assert!(matches!(
            validate_resource_limits(&req),
            Err(ApiError::BadRequest(m)) if m.contains("memory_mb")
        ));
    }

    #[test]
    fn test_validate_resource_limits_rejects_excessive_memory() {
        let mut req = create_run_request(RunKind::Preview);
        req.memory_mb = Some(MEMORY_MAX + 1);
        assert!(matches!(
            validate_resource_limits(&req),
            Err(ApiError::BadRequest(m)) if m.contains("memory_mb")
        ));
    }

    #[test]
    fn test_validate_resource_limits_rejects_negative_disk() {
        let mut req = create_run_request(RunKind::Preview);
        req.disk_mb = Some(0);
        assert!(matches!(
            validate_resource_limits(&req),
            Err(ApiError::BadRequest(m)) if m.contains("disk_mb")
        ));
    }

    #[test]
    fn test_validate_resource_limits_rejects_excessive_disk() {
        let mut req = create_run_request(RunKind::Preview);
        req.disk_mb = Some(DISK_MAX + 1);
        assert!(matches!(
            validate_resource_limits(&req),
            Err(ApiError::BadRequest(m)) if m.contains("disk_mb")
        ));
    }

    #[test]
    fn test_validate_resource_limits_rejects_negative_priority() {
        let mut req = create_run_request(RunKind::Preview);
        req.priority = Some(-1);
        assert!(matches!(
            validate_resource_limits(&req),
            Err(ApiError::BadRequest(m)) if m.contains("priority")
        ));
    }
}
