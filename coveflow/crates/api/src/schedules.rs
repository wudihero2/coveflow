//! Cron schedule CRUD + trigger endpoints.
//!
//! A schedule fires a flow on a cron expression. Flows are referenced by stable
//! `flow_id`; the current path is resolved on demand for ACL + display. Managing
//! a schedule requires write on the flow's folder, viewing requires read.
//! Scheduled (and run-now) runs execute as the schedule's creator.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use coveflow_types::RunKind;
use coveflow_types::schedule::Schedule;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::AuthedUser;
use crate::error::ApiError;

const DEFAULT_TZ: &str = "UTC";

#[derive(Serialize)]
pub struct LastRun {
    pub id: Uuid,
    pub created_at: DateTime<Utc>,
    /// `Some(true/false)` once terminal; `None` while queued/running.
    pub success: Option<bool>,
}

#[derive(Serialize)]
pub struct ScheduleListItem {
    pub id: Uuid,
    pub name: String,
    pub flow_id: Uuid,
    /// Current path of the flow (resolved from `flow_id`), for display.
    pub flow_path: String,
    pub cron_expr: String,
    pub timezone: String,
    pub enabled: bool,
    pub catchup: bool,
    pub max_active_runs: Option<i32>,
    pub next_trigger_at: Option<DateTime<Utc>>,
    pub last_triggered_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub last_run: Option<LastRun>,
}

/// Full schedule + the flow's current path (resolved from `flow_id`), for the
/// detail view.
#[derive(Serialize)]
pub struct ScheduleDetail {
    #[serde(flatten)]
    pub schedule: Schedule,
    pub flow_path: String,
}

#[derive(Deserialize)]
pub struct CreateScheduleRequest {
    pub name: String,
    pub flow_id: Uuid,
    pub cron_expr: String,
    #[serde(default)]
    pub timezone: Option<String>,
    #[serde(default)]
    pub args: Option<serde_json::Value>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub catchup: Option<bool>,
    #[serde(default)]
    pub max_active_runs: Option<i32>,
}

/// Full-replace update (PUT semantics): same shape as create.
pub type UpdateScheduleRequest = CreateScheduleRequest;

#[derive(Deserialize)]
pub struct EnableScheduleRequest {
    pub enabled: bool,
}

#[derive(Deserialize)]
pub struct PreviewRequest {
    pub cron_expr: String,
    #[serde(default)]
    pub timezone: Option<String>,
    #[serde(default)]
    pub count: Option<usize>,
}

#[derive(Serialize)]
pub struct PreviewResponse {
    pub next: Vec<DateTime<Utc>>,
}

#[derive(Serialize)]
pub struct RunCreated {
    pub id: String,
}

// --- helpers ---------------------------------------------------------------

fn validate_cron(cron_expr: &str, tz: &str) -> Result<(), ApiError> {
    coveflow_queue::validate_cron(cron_expr, tz).map_err(|e| ApiError::BadRequest(e.to_string()))
}

fn compute_next(cron_expr: &str, tz: &str) -> Result<Option<DateTime<Utc>>, ApiError> {
    coveflow_queue::next_after(cron_expr, tz, Utc::now())
        .map_err(|e| ApiError::BadRequest(e.to_string()))
}

/// Map a unique-constraint violation to 409, everything else to its default.
fn map_unique(e: sqlx::Error, msg: &str) -> ApiError {
    match &e {
        sqlx::Error::Database(d) if d.is_unique_violation() => ApiError::Conflict(msg.to_string()),
        _ => ApiError::from(e),
    }
}

/// Current path of a logical flow (latest revision) by stable id, or `None` if it
/// no longer exists. The path is a movable label, so every schedule op resolves it
/// from `flow_id` for ACL + display.
pub(crate) async fn current_flow_path(
    db: &PgPool,
    workspace_id: &str,
    flow_id: Uuid,
) -> Result<Option<String>, ApiError> {
    let path = sqlx::query_scalar!(
        "SELECT path FROM flow WHERE workspace_id = $1 AND flow_id = $2
         ORDER BY revision DESC LIMIT 1",
        workspace_id,
        flow_id
    )
    .fetch_optional(db)
    .await?;
    Ok(path)
}

async fn load_schedule(db: &PgPool, workspace_id: &str, id: Uuid) -> Result<Schedule, ApiError> {
    sqlx::query_as!(
        Schedule,
        r#"SELECT id, workspace_id, name, flow_id, cron_expr, timezone, args, enabled,
                  catchup, max_active_runs, next_trigger_at, last_triggered_at, last_error,
                  created_by, created_at, updated_at
           FROM schedule WHERE workspace_id = $1 AND id = $2"#,
        workspace_id,
        id
    )
    .fetch_optional(db)
    .await?
    .ok_or(ApiError::NotFound)
}

fn validate_request(req: &CreateScheduleRequest, tz: &str) -> Result<(), ApiError> {
    if req.name.trim().is_empty() {
        return Err(ApiError::BadRequest("name must not be empty".into()));
    }
    validate_cron(&req.cron_expr, tz)?;
    if let Some(m) = req.max_active_runs {
        if m <= 0 {
            return Err(ApiError::BadRequest(
                "max_active_runs must be > 0 (omit for unlimited)".into(),
            ));
        }
    }
    Ok(())
}

// --- handlers --------------------------------------------------------------

#[tracing::instrument(name = "api::list_schedules", skip(db, user), fields(%workspace_id))]
pub async fn list_schedules(
    State(db): State<PgPool>,
    axum::Extension(user): axum::Extension<AuthedUser>,
    Path(workspace_id): Path<String>,
) -> Result<Json<Vec<ScheduleListItem>>, ApiError> {
    let rows = sqlx::query!(
        r#"SELECT s.id, s.name, s.flow_id, s.cron_expr, s.timezone, s.enabled,
                  s.catchup, s.max_active_runs, s.next_trigger_at, s.last_triggered_at,
                  s.last_error,
                  f.path AS "flow_path?",
                  lr.id AS "last_run_id?", lr.created_at AS "last_run_at?",
                  c.success AS "last_run_success?"
           FROM schedule s
           LEFT JOIN LATERAL (
               SELECT path FROM flow
               WHERE workspace_id = s.workspace_id AND flow_id = s.flow_id
               ORDER BY revision DESC LIMIT 1
           ) f ON TRUE
           LEFT JOIN LATERAL (
               SELECT id, created_at FROM run
               WHERE schedule_id = s.id ORDER BY created_at DESC LIMIT 1
           ) lr ON TRUE
           LEFT JOIN run_completed c ON c.id = lr.id
           WHERE s.workspace_id = $1
           ORDER BY s.name"#,
        workspace_id
    )
    .fetch_all(&db)
    .await?;

    let items = rows
        .into_iter()
        // Resolve current path for display + ACL. An unresolvable flow_id (no current
        // path) is an orphan that shouldn't occur — delete_flow cascades its schedules,
        // rename is id-stable, and overwrite-move cascades the replaced flow's schedules.
        // Hide it rather than show it: path-based ACL has no basis without a path, so
        // surfacing it would leak the name/cron of a flow the caller may not be able to read.
        .filter_map(|r| {
            let flow_path = r.flow_path?;
            if !user.can_read(&flow_path) {
                return None;
            }
            Some(ScheduleListItem {
                id: r.id,
                name: r.name,
                flow_id: r.flow_id,
                flow_path,
                cron_expr: r.cron_expr,
                timezone: r.timezone,
                enabled: r.enabled,
                catchup: r.catchup,
                max_active_runs: r.max_active_runs,
                next_trigger_at: r.next_trigger_at,
                last_triggered_at: r.last_triggered_at,
                last_error: r.last_error,
                last_run: r.last_run_id.map(|id| LastRun {
                    id,
                    created_at: r.last_run_at.unwrap_or_else(Utc::now),
                    success: r.last_run_success,
                }),
            })
        })
        .collect();
    Ok(Json(items))
}

#[tracing::instrument(name = "api::create_schedule", skip(db, user, req), fields(%workspace_id))]
pub async fn create_schedule(
    State(db): State<PgPool>,
    axum::Extension(user): axum::Extension<AuthedUser>,
    Path(workspace_id): Path<String>,
    Json(req): Json<CreateScheduleRequest>,
) -> Result<Response, ApiError> {
    let flow_path = current_flow_path(&db, &workspace_id, req.flow_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest(format!("flow '{}' not found", req.flow_id)))?;
    user.require_writer(&flow_path)?;
    let tz = req.timezone.clone().unwrap_or_else(|| DEFAULT_TZ.into());
    validate_request(&req, &tz)?;
    let enabled = req.enabled.unwrap_or(true);
    let next = if enabled {
        compute_next(&req.cron_expr, &tz)?
    } else {
        None
    };
    let args = req.args.unwrap_or_else(|| serde_json::json!({}));

    let id = sqlx::query_scalar!(
        "INSERT INTO schedule (workspace_id, name, flow_id, cron_expr, timezone, args,
             enabled, catchup, max_active_runs, next_trigger_at, created_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) RETURNING id",
        workspace_id,
        req.name.trim(),
        req.flow_id,
        req.cron_expr,
        tz,
        args,
        enabled,
        req.catchup.unwrap_or(false),
        req.max_active_runs,
        next,
        user.email,
    )
    .fetch_one(&db)
    .await
    .map_err(|e| map_unique(e, "a schedule with this name already exists for this flow"))?;

    Ok((StatusCode::CREATED, Json(serde_json::json!({ "id": id }))).into_response())
}

#[tracing::instrument(name = "api::get_schedule", skip(db, user), fields(%workspace_id))]
pub async fn get_schedule(
    State(db): State<PgPool>,
    axum::Extension(user): axum::Extension<AuthedUser>,
    Path((workspace_id, id)): Path<(String, Uuid)>,
) -> Result<Json<ScheduleDetail>, ApiError> {
    let s = load_schedule(&db, &workspace_id, id).await?;
    let flow_path = current_flow_path(&db, &workspace_id, s.flow_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    user.require_reader(&flow_path)?;
    Ok(Json(ScheduleDetail {
        schedule: s,
        flow_path,
    }))
}

#[tracing::instrument(name = "api::update_schedule", skip(db, user, req), fields(%workspace_id))]
pub async fn update_schedule(
    State(db): State<PgPool>,
    axum::Extension(user): axum::Extension<AuthedUser>,
    Path((workspace_id, id)): Path<(String, Uuid)>,
    Json(req): Json<UpdateScheduleRequest>,
) -> Result<Response, ApiError> {
    let existing = load_schedule(&db, &workspace_id, id).await?;
    // Need write on both the old and (possibly changed) new flow folder, resolved
    // from their stable ids.
    let old_path = current_flow_path(&db, &workspace_id, existing.flow_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    user.require_writer(&old_path)?;
    let new_path = current_flow_path(&db, &workspace_id, req.flow_id)
        .await?
        .ok_or_else(|| ApiError::BadRequest(format!("flow '{}' not found", req.flow_id)))?;
    user.require_writer(&new_path)?;
    let tz = req.timezone.clone().unwrap_or_else(|| DEFAULT_TZ.into());
    validate_request(&req, &tz)?;
    let enabled = req.enabled.unwrap_or(true);
    let next = if enabled {
        compute_next(&req.cron_expr, &tz)?
    } else {
        None
    };
    let args = req.args.unwrap_or_else(|| serde_json::json!({}));

    sqlx::query!(
        "UPDATE schedule SET name = $1, flow_id = $2, cron_expr = $3, timezone = $4,
             args = $5, enabled = $6, catchup = $7, max_active_runs = $8,
             next_trigger_at = $9, last_error = NULL, updated_at = now()
         WHERE workspace_id = $10 AND id = $11",
        req.name.trim(),
        req.flow_id,
        req.cron_expr,
        tz,
        args,
        enabled,
        req.catchup.unwrap_or(false),
        req.max_active_runs,
        next,
        workspace_id,
        id,
    )
    .execute(&db)
    .await
    .map_err(|e| map_unique(e, "a schedule with this name already exists for this flow"))?;

    Ok((StatusCode::OK, Json(serde_json::json!({ "id": id }))).into_response())
}

#[tracing::instrument(name = "api::delete_schedule", skip(db, user), fields(%workspace_id))]
pub async fn delete_schedule(
    State(db): State<PgPool>,
    axum::Extension(user): axum::Extension<AuthedUser>,
    Path((workspace_id, id)): Path<(String, Uuid)>,
) -> Result<Response, ApiError> {
    let existing = load_schedule(&db, &workspace_id, id).await?;
    let flow_path = current_flow_path(&db, &workspace_id, existing.flow_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    user.require_writer(&flow_path)?;
    sqlx::query!(
        "DELETE FROM schedule WHERE workspace_id = $1 AND id = $2",
        workspace_id,
        id
    )
    .execute(&db)
    .await?;
    Ok((StatusCode::OK, Json(serde_json::json!({ "deleted": true }))).into_response())
}

#[tracing::instrument(name = "api::enable_schedule", skip(db, user, req), fields(%workspace_id))]
pub async fn enable_schedule(
    State(db): State<PgPool>,
    axum::Extension(user): axum::Extension<AuthedUser>,
    Path((workspace_id, id)): Path<(String, Uuid)>,
    Json(req): Json<EnableScheduleRequest>,
) -> Result<Response, ApiError> {
    let existing = load_schedule(&db, &workspace_id, id).await?;
    let flow_path = current_flow_path(&db, &workspace_id, existing.flow_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    user.require_writer(&flow_path)?;
    // Disabling preserves next_trigger_at (the scheduler ignores disabled rows
    // anyway). On enable: a `catchup` schedule resumes from the preserved point
    // so the pause is backfilled like any other missed window (capped by the
    // scheduler's MAX_CATCHUP); otherwise it skips ahead to the next future tick.
    let next = if req.enabled {
        match existing.next_trigger_at {
            Some(prev) if existing.catchup => Some(prev),
            _ => compute_next(&existing.cron_expr, &existing.timezone)?,
        }
    } else {
        existing.next_trigger_at
    };
    sqlx::query!(
        "UPDATE schedule SET enabled = $1, next_trigger_at = $2, last_error = NULL, updated_at = now()
         WHERE workspace_id = $3 AND id = $4",
        req.enabled,
        next,
        workspace_id,
        id,
    )
    .execute(&db)
    .await?;
    Ok((
        StatusCode::OK,
        Json(serde_json::json!({ "enabled": req.enabled })),
    )
        .into_response())
}

/// Preview the next N trigger instants for a cron + timezone. Stateless; powers
/// the editor's live preview, sharing the scheduler's cron parser.
#[tracing::instrument(name = "api::preview_schedule", skip(_user, req), fields(%workspace_id))]
pub async fn preview_schedule(
    axum::Extension(_user): axum::Extension<AuthedUser>,
    Path(workspace_id): Path<String>,
    Json(req): Json<PreviewRequest>,
) -> Result<Json<PreviewResponse>, ApiError> {
    let tz = req.timezone.unwrap_or_else(|| DEFAULT_TZ.into());
    let count = req.count.unwrap_or(5).clamp(1, 20);
    let next = coveflow_queue::upcoming(&req.cron_expr, &tz, Utc::now(), count)
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    Ok(Json(PreviewResponse { next }))
}

/// Run a schedule's flow immediately (does not affect `next_trigger_at`). Uses
/// the schedule's args + creator, same as a cron-fired run.
#[tracing::instrument(name = "api::run_schedule_now", skip(db, user), fields(%workspace_id))]
pub async fn run_schedule_now(
    State(db): State<PgPool>,
    axum::Extension(user): axum::Extension<AuthedUser>,
    Path((workspace_id, id)): Path<(String, Uuid)>,
) -> Result<Response, ApiError> {
    let s = load_schedule(&db, &workspace_id, id).await?;
    // Resolve the flow by stable id (latest revision), capturing its current path.
    let flow = sqlx::query!(
        "SELECT value, path FROM flow WHERE workspace_id = $1 AND flow_id = $2
         ORDER BY revision DESC LIMIT 1",
        workspace_id,
        s.flow_id
    )
    .fetch_optional(&db)
    .await?
    .ok_or_else(|| ApiError::BadRequest(format!("flow '{}' not found", s.flow_id)))?;
    let value = flow.value;
    let flow_path = flow.path;
    user.require_writer(&flow_path)?;

    let run_id = coveflow_queue::submit_run(
        &db,
        coveflow_queue::NewRun {
            workspace_id: &workspace_id,
            kind: RunKind::Flow,
            script_hash: None,
            script_path: Some(&flow_path),
            raw_code: None,
            language: None,
            args: Some(s.args.clone()),
            flow_value: Some(value),
            tag: "default",
            parent_run: None,
            root_run: None,
            flow_step_id: None,
            team_owner: None,
            created_by: &s.created_by,
            trace_id: None,
            span_id: None,
            scheduled_for: None,
            priority: None,
            cpus: None,
            memory_mb: None,
            disk_mb: None,
            requirements: vec![],
            timeout: None,
            custom_image: None,
            schedule_id: Some(s.id),
            scheduled_time: None,
            data_interval_end: None,
            trigger_id: None,
            trigger_context: None,
        },
    )
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(RunCreated {
            id: run_id.to_string(),
        }),
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use serde_json::Value;
    use sqlx::PgPool;
    use tower::ServiceExt;

    use crate::test_helpers::*;

    async fn seed_flow(db: &PgPool, ws: &str, path: &str) -> uuid::Uuid {
        sqlx::query_scalar!(
            "INSERT INTO flow (workspace_id, path, revision, summary, value, edited_by, flow_id)
             VALUES ($1, $2, 1, '', $3, 'u@test.local', gen_random_uuid())
             RETURNING flow_id",
            ws,
            path,
            serde_json::json!({ "nodes": [{ "id": "a", "body": { "kind": "script", "script_id": "11111111-1111-1111-1111-111111111111" } }], "edges": [] })
        )
        .fetch_one(db)
        .await
        .unwrap()
    }

    fn app(pool: &PgPool) -> axum::Router {
        crate::create_router(
            pool.clone(),
            test_metrics(),
            crate::test_helpers::test_secret_key(),
        )
    }

    fn req(method: &str, uri: &str, token: &str, body: &str) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    async fn body_json(resp: axum::response::Response) -> Value {
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    }

    async fn create(
        app: &axum::Router,
        token: &str,
        name: &str,
        flow_id: uuid::Uuid,
        cron: &str,
    ) -> axum::response::Response {
        let body = serde_json::json!({ "name": name, "flow_id": flow_id, "cron_expr": cron });
        app.clone()
            .oneshot(req(
                "POST",
                "/api/workspaces/ws-1/schedules/create",
                token,
                &body.to_string(),
            ))
            .await
            .unwrap()
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn create_list_get(pool: PgPool) {
        let user = "dev@test.local";
        seed_account(&pool, user).await;
        seed_workspace_member(&pool, "ws-1", user, "admin").await;
        let fid = seed_flow(&pool, "ws-1", "workspace/f").await;
        let token = valid_jwt(user);
        let app = app(&pool);

        let resp = create(&app, &token, "nightly", fid, "0 2 * * *").await;
        assert_eq!(resp.status(), StatusCode::CREATED);
        let id = body_json(resp).await["id"].as_str().unwrap().to_string();

        let resp = app
            .clone()
            .oneshot(req(
                "GET",
                "/api/workspaces/ws-1/schedules/list",
                &token,
                "",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let list = body_json(resp).await;
        assert_eq!(list.as_array().unwrap().len(), 1);
        assert_eq!(list[0]["name"], "nightly");
        assert!(
            list[0]["next_trigger_at"].is_string(),
            "enabled → has next trigger"
        );

        let resp = app
            .clone()
            .oneshot(req(
                "GET",
                &format!("/api/workspaces/ws-1/schedules/get/{id}"),
                &token,
                "",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(body_json(resp).await["cron_expr"], "0 2 * * *");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn create_requires_write(pool: PgPool) {
        let user = "viewer@test.local";
        seed_account(&pool, user).await;
        seed_workspace_member(&pool, "ws-1", user, "viewer").await;
        let fid = seed_flow(&pool, "ws-1", "workspace/f").await;
        let token = valid_jwt(user);
        let resp = create(&app(&pool), &token, "s", fid, "0 2 * * *").await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn bad_cron_rejected(pool: PgPool) {
        let user = "dev@test.local";
        seed_account(&pool, user).await;
        seed_workspace_member(&pool, "ws-1", user, "admin").await;
        let fid = seed_flow(&pool, "ws-1", "workspace/f").await;
        let resp = create(&app(&pool), &valid_jwt(user), "s", fid, "not a cron").await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn missing_flow_rejected(pool: PgPool) {
        let user = "dev@test.local";
        seed_account(&pool, user).await;
        seed_workspace_member(&pool, "ws-1", user, "admin").await;
        // A flow_id that does not exist → 400.
        let resp = create(
            &app(&pool),
            &valid_jwt(user),
            "s",
            uuid::Uuid::new_v4(),
            "0 2 * * *",
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn duplicate_name_conflicts(pool: PgPool) {
        let user = "dev@test.local";
        seed_account(&pool, user).await;
        seed_workspace_member(&pool, "ws-1", user, "admin").await;
        let fid = seed_flow(&pool, "ws-1", "workspace/f").await;
        let token = valid_jwt(user);
        let app = app(&pool);
        assert_eq!(
            create(&app, &token, "dup", fid, "0 2 * * *").await.status(),
            StatusCode::CREATED
        );
        assert_eq!(
            create(&app, &token, "dup", fid, "0 3 * * *").await.status(),
            StatusCode::CONFLICT
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn preview_returns_next(pool: PgPool) {
        let user = "dev@test.local";
        seed_account(&pool, user).await;
        seed_workspace_member(&pool, "ws-1", user, "admin").await;
        let app = app(&pool);
        let token = valid_jwt(user);

        let body = serde_json::json!({ "cron_expr": "0 2 * * *", "timezone": "UTC", "count": 3 });
        let resp = app
            .clone()
            .oneshot(req(
                "POST",
                "/api/workspaces/ws-1/schedules/preview",
                &token,
                &body.to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(body_json(resp).await["next"].as_array().unwrap().len(), 3);

        let bad = serde_json::json!({ "cron_expr": "nope" });
        let resp = app
            .oneshot(req(
                "POST",
                "/api/workspaces/ws-1/schedules/preview",
                &token,
                &bad.to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn run_now_creates_run_with_schedule_id(pool: PgPool) {
        let user = "dev@test.local";
        seed_account(&pool, user).await;
        seed_workspace_member(&pool, "ws-1", user, "admin").await;
        let fid = seed_flow(&pool, "ws-1", "workspace/f").await;
        let token = valid_jwt(user);
        let app = app(&pool);

        let id = body_json(create(&app, &token, "s", fid, "0 2 * * *").await).await["id"]
            .as_str()
            .unwrap()
            .to_string();

        let resp = app
            .oneshot(req(
                "POST",
                &format!("/api/workspaces/ws-1/schedules/{id}/run"),
                &token,
                "",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        // The run carries the schedule_id (history is viewed via /runs?schedule_id).
        let n = sqlx::query_scalar!(
            r#"SELECT count(*) AS "n!" FROM run WHERE schedule_id = $1::uuid"#,
            id.parse::<uuid::Uuid>().unwrap()
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(n, 1);
    }

    async fn make_schedule(pool: &PgPool, token: &str, app: &axum::Router) -> String {
        let fid = seed_flow(pool, "ws-1", "workspace/f").await;
        body_json(create(app, token, "s", fid, "0 2 * * *").await).await["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    async fn enable(app: &axum::Router, token: &str, id: &str, enabled: bool) {
        let body = format!(r#"{{"enabled":{enabled}}}"#);
        let resp = app
            .clone()
            .oneshot(req(
                "POST",
                &format!("/api/workspaces/ws-1/schedules/{id}/enable"),
                token,
                &body,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// Force "was disabled while a tick came due": past next_trigger_at + chosen
    /// catchup, disabled.
    async fn force_overdue(pool: &PgPool, id: &str, catchup: bool) {
        sqlx::query!(
            "UPDATE schedule SET catchup = $1, enabled = FALSE,
                 next_trigger_at = now() - interval '1 day' WHERE id = $2::uuid",
            catchup,
            id.parse::<uuid::Uuid>().unwrap()
        )
        .execute(pool)
        .await
        .unwrap();
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn disable_preserves_next_trigger(pool: PgPool) {
        let user = "dev@test.local";
        seed_account(&pool, user).await;
        seed_workspace_member(&pool, "ws-1", user, "admin").await;
        let token = valid_jwt(user);
        let app = app(&pool);
        let id = make_schedule(&pool, &token, &app).await;

        let before = sqlx::query_scalar!(
            "SELECT next_trigger_at FROM schedule WHERE id = $1::uuid",
            id.parse::<uuid::Uuid>().unwrap()
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        enable(&app, &token, &id, false).await;
        let after = sqlx::query_scalar!(
            "SELECT next_trigger_at FROM schedule WHERE id = $1::uuid",
            id.parse::<uuid::Uuid>().unwrap()
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        // Disable does not clear next_trigger_at (scheduler ignores disabled rows).
        assert!(after.is_some());
        assert_eq!(before, after);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn enable_with_catchup_resumes_from_past(pool: PgPool) {
        let user = "dev@test.local";
        seed_account(&pool, user).await;
        seed_workspace_member(&pool, "ws-1", user, "admin").await;
        let token = valid_jwt(user);
        let app = app(&pool);
        let id = make_schedule(&pool, &token, &app).await;
        force_overdue(&pool, &id, true).await; // catchup ON

        enable(&app, &token, &id, true).await;
        // Resumes from the preserved past point → still in the past → scheduler
        // backfills the paused window on the next tick.
        let row = sqlx::query!(
            r#"SELECT next_trigger_at AS "next!", now() AS "now!" FROM schedule WHERE id = $1::uuid"#,
            id.parse::<uuid::Uuid>().unwrap()
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(row.next < row.now, "catchup enable keeps the overdue point");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn enable_without_catchup_skips_to_future(pool: PgPool) {
        let user = "dev@test.local";
        seed_account(&pool, user).await;
        seed_workspace_member(&pool, "ws-1", user, "admin").await;
        let token = valid_jwt(user);
        let app = app(&pool);
        let id = make_schedule(&pool, &token, &app).await;
        force_overdue(&pool, &id, false).await; // catchup OFF

        enable(&app, &token, &id, true).await;
        let row = sqlx::query!(
            r#"SELECT next_trigger_at AS "next!", now() AS "now!" FROM schedule WHERE id = $1::uuid"#,
            id.parse::<uuid::Uuid>().unwrap()
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(
            row.next > row.now,
            "no-catchup enable skips the paused window"
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn delete_removes(pool: PgPool) {
        let user = "dev@test.local";
        seed_account(&pool, user).await;
        seed_workspace_member(&pool, "ws-1", user, "admin").await;
        let fid = seed_flow(&pool, "ws-1", "workspace/f").await;
        let token = valid_jwt(user);
        let app = app(&pool);
        let id = body_json(create(&app, &token, "s", fid, "0 2 * * *").await).await["id"]
            .as_str()
            .unwrap()
            .to_string();
        let resp = app
            .oneshot(req(
                "DELETE",
                &format!("/api/workspaces/ws-1/schedules/{id}"),
                &token,
                "",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let n = sqlx::query_scalar!(r#"SELECT count(*) AS "n!" FROM schedule"#)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn moving_flow_rewrites_schedule_path(pool: PgPool) {
        let user = "dev@test.local";
        seed_account(&pool, user).await;
        seed_workspace_member(&pool, "ws-1", user, "admin").await;
        let fid = seed_flow(&pool, "ws-1", "workspace/f").await;
        let token = valid_jwt(user);
        let app = app(&pool);
        let id = body_json(create(&app, &token, "s", fid, "0 2 * * *").await).await["id"]
            .as_str()
            .unwrap()
            .to_string();

        let body = serde_json::json!({ "old_path": "workspace/f", "new_path": "workspace/g" });
        let resp = app
            .clone()
            .oneshot(req(
                "POST",
                "/api/workspaces/ws-1/flows/move",
                &token,
                &body.to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let resp = app
            .oneshot(req(
                "GET",
                &format!("/api/workspaces/ws-1/schedules/get/{id}"),
                &token,
                "",
            ))
            .await
            .unwrap();
        assert_eq!(body_json(resp).await["flow_path"], "workspace/g");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn deleting_referenced_flow_blocked_then_forced(pool: PgPool) {
        let user = "dev@test.local";
        seed_account(&pool, user).await;
        seed_workspace_member(&pool, "ws-1", user, "admin").await;
        let fid = seed_flow(&pool, "ws-1", "workspace/f").await;
        let token = valid_jwt(user);
        let app = app(&pool);
        create(&app, &token, "s", fid, "0 2 * * *").await;

        // Blocked: a schedule references the flow.
        let resp = app
            .clone()
            .oneshot(req(
                "POST",
                "/api/workspaces/ws-1/flows/delete",
                &token,
                r#"{"path":"workspace/f"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);

        // Forced: deletes the flow and its schedules.
        let resp = app
            .oneshot(req(
                "POST",
                "/api/workspaces/ws-1/flows/delete",
                &token,
                r#"{"path":"workspace/f","force":true}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let n = sqlx::query_scalar!(r#"SELECT count(*) AS "n!" FROM schedule"#)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0, "schedules cascade-deleted on force");
    }
}
