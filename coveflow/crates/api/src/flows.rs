//! Flow CRUD + trigger endpoints.
//!
//! Flows are stored in the `flow` table (one row per revision). Triggering a
//! flow submits a `kind='flow'` run whose `flow_value` is the stored definition;
//! the flow engine ([`coveflow_queue::advance_flow`]) drives it from there.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use coveflow_types::RunKind;
use coveflow_types::flows::{Expr, FlowNode, FlowSpec, InputBinding, NodeBody, NodeId};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::auth::AuthedUser;
use crate::error::ApiError;

#[derive(Deserialize)]
pub struct CreateFlowRequest {
    pub path: String,
    /// The flow definition (a `FlowSpec`).
    pub value: serde_json::Value,
    #[serde(default)]
    pub summary: Option<String>,
}

#[derive(Serialize)]
pub struct FlowResponse {
    pub flow_id: uuid::Uuid,
    pub path: String,
    pub revision: i32,
    pub summary: String,
    pub value: serde_json::Value,
    pub edited_by: String,
    pub edited_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Serialize)]
pub struct FlowListItem {
    pub flow_id: uuid::Uuid,
    pub path: String,
    pub revision: i32,
    pub summary: String,
    pub edited_by: String,
    pub edited_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Deserialize)]
pub struct RunFlowRequest {
    #[serde(default)]
    pub args: Option<serde_json::Value>,
}

#[derive(Serialize)]
pub struct RunCreated {
    pub id: String,
}

#[derive(Deserialize)]
pub struct CheckExprRequest {
    pub expr: String,
}

#[derive(Serialize)]
pub struct CheckExprResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Validate a single expression without saving anything. Powers the live
/// "is this expression valid?" feedback in the flow editor. Always returns 200;
/// validity is in the body so the client can render inline pass/fail.
#[tracing::instrument(name = "api::check_flow_expr", skip(_user, req), fields(%workspace_id))]
pub async fn check_flow_expr(
    axum::Extension(_user): axum::Extension<AuthedUser>,
    Path(workspace_id): Path<String>,
    Json(req): Json<CheckExprRequest>,
) -> Result<Response, ApiError> {
    let resp = match coveflow_flow_expr::check(&req.expr) {
        Ok(()) => CheckExprResponse {
            ok: true,
            error: None,
        },
        Err(e) => CheckExprResponse {
            ok: false,
            error: Some(e.to_string()),
        },
    };
    Ok(Json(resp).into_response())
}

/// Parse + validate a flow definition (structure and embedded expressions).
fn parse_and_validate(value: &serde_json::Value) -> Result<FlowSpec, ApiError> {
    let spec: FlowSpec = serde_json::from_value(value.clone())
        .map_err(|e| ApiError::BadRequest(format!("invalid flow definition: {e}")))?;
    if let Err(errs) = spec.validate() {
        let msg = errs
            .iter()
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
            .join("; ");
        return Err(ApiError::BadRequest(format!("invalid flow: {msg}")));
    }
    for node in &spec.nodes {
        validate_node_exprs(node)?;
    }
    if let Some(handler) = &spec.on_error {
        validate_node_exprs(handler)?;
    }
    for e in &spec.edges {
        if let Some(when) = &e.when {
            check_expr_at(&format!("edge {} → {} · condition", e.from, e.to), when)?;
        }
    }
    Ok(spec)
}

/// `loc` names where the expression lives (e.g. `node 'a0' · input 'ctx'`) so the
/// 400 tells the user exactly which field to fix, not just that *something* is bad.
fn check_expr_at(loc: &str, e: &Expr) -> Result<(), ApiError> {
    coveflow_flow_expr::check(&e.0).map_err(|err| {
        let what = if e.0.trim().is_empty() {
            "expression is empty".to_string()
        } else {
            format!("invalid expression '{}': {err}", e.0)
        };
        ApiError::BadRequest(format!("{loc}: {what}"))
    })
}

fn check_binding_at(loc: &str, b: &InputBinding) -> Result<(), ApiError> {
    match b {
        InputBinding::Expr { expr } => check_expr_at(loc, expr),
        InputBinding::Static { .. } => Ok(()),
    }
}

/// Check that every embedded expression in a node parses, so typos are caught at
/// save time rather than mid-run. Errors name the node + field so the editor can
/// point the user at the exact place to fix.
fn validate_node_exprs(node: &FlowNode) -> Result<(), ApiError> {
    if let Some(e) = &node.skip_if {
        check_expr_at(&format!("node '{}' · skip condition", node.id), e)?;
    }
    check_body_exprs(&node.id, &node.body)
}

fn check_body_exprs(node_id: &NodeId, body: &NodeBody) -> Result<(), ApiError> {
    match body {
        NodeBody::Script { inputs, .. } => {
            for (key, b) in inputs {
                check_binding_at(&format!("node '{node_id}' · input '{key}'"), b)?;
            }
        }
        NodeBody::Branch { task } => {
            check_body_exprs(node_id, task)?;
        }
    }
    Ok(())
}

#[tracing::instrument(name = "api::create_flow", skip(db, user, req), fields(%workspace_id, path = %req.path))]
pub async fn create_flow(
    State(db): State<PgPool>,
    axum::Extension(user): axum::Extension<AuthedUser>,
    Path(workspace_id): Path<String>,
    Json(req): Json<CreateFlowRequest>,
) -> Result<Response, ApiError> {
    if req.path.is_empty() {
        return Err(ApiError::BadRequest("path must not be empty".into()));
    }
    if req.path.ends_with('/') {
        return Err(ApiError::BadRequest(
            "flow name is required (the path must end with a name, not '/')".into(),
        ));
    }
    if !user.is_valid_root_path(&req.path) {
        return Err(ApiError::BadRequest(
            "path must be under users/<you>/, teams/<your team>/, or workspace/".into(),
        ));
    }
    user.require_writer(&req.path)?;
    // Re-serialize from the validated spec so stored JSON is canonical.
    let spec = parse_and_validate(&req.value)?;
    let canonical = serde_json::to_value(&spec)
        .map_err(|e| ApiError::Internal(format!("serialize flow: {e}")))?;
    let summary = req.summary.unwrap_or_default();

    // Stable per-logical-flow id: inherit the existing id for this path (a new
    // revision of the same flow), or mint one for a brand-new path.
    let existing = sqlx::query!(
        r#"SELECT flow_id, COALESCE(MAX(revision) OVER (), 0) AS "max_rev!"
           FROM flow WHERE workspace_id = $1 AND path = $2
           ORDER BY revision DESC LIMIT 1"#,
        workspace_id,
        req.path,
    )
    .fetch_optional(&db)
    .await?;
    let (flow_id, next_revision) = match existing {
        Some(r) => (r.flow_id, r.max_rev + 1),
        None => (uuid::Uuid::new_v4(), 1),
    };

    let revision = sqlx::query_scalar!(
        r#"INSERT INTO flow (workspace_id, path, revision, summary, value, edited_by, flow_id)
           VALUES ($1, $2, $3, $4, $5, $6, $7)
           RETURNING revision"#,
        workspace_id,
        req.path,
        next_revision,
        summary,
        canonical,
        user.email,
        flow_id,
    )
    .fetch_one(&db)
    .await?;

    Ok((
        axum::http::StatusCode::CREATED,
        Json(serde_json::json!({ "path": req.path, "revision": revision, "flow_id": flow_id })),
    )
        .into_response())
}

#[derive(Deserialize)]
pub struct MoveFlowRequest {
    pub old_path: String,
    pub new_path: String,
    /// Replace an existing flow at the target path (requires write there).
    #[serde(default)]
    pub overwrite: bool,
}

/// Move/rename a flow: rewrite `path` on every revision of `(workspace,
/// old_path)`. The flow's `flow_id` is stable across the rename, so schedules
/// (which reference `flow_id`) need no change; runs carry their own `flow_value`
/// snapshot. On `overwrite`, the replaced target flow's schedules are deleted
/// with it. Requires write on both the source and destination folders.
#[tracing::instrument(name = "api::move_flow", skip(db, user, req), fields(%workspace_id))]
pub async fn move_flow(
    State(db): State<PgPool>,
    axum::Extension(user): axum::Extension<AuthedUser>,
    Path(workspace_id): Path<String>,
    Json(req): Json<MoveFlowRequest>,
) -> Result<Response, ApiError> {
    let old_path = req.old_path.trim().to_string();
    let new_path = req.new_path.trim().to_string();
    if new_path.is_empty() || new_path.ends_with('/') {
        return Err(ApiError::BadRequest(
            "new_path must be a non-empty path".into(),
        ));
    }
    if !user.is_valid_root_path(&new_path) {
        return Err(ApiError::BadRequest(
            "new_path must be under users/<you>/, teams/<your team>/, or workspace/".into(),
        ));
    }
    user.require_writer(&old_path)?;
    user.require_writer(&new_path)?;
    if old_path == new_path {
        return Ok((
            StatusCode::OK,
            Json(serde_json::json!({ "path": new_path })),
        )
            .into_response());
    }

    let mut tx = db.begin().await?;
    // Serialize the collision check + write against concurrent create/move.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1 || ':' || $2)::bigint)")
        .bind(&workspace_id)
        .bind(&new_path)
        .execute(&mut *tx)
        .await?;

    let source_exists: bool = sqlx::query_scalar!(
        r#"SELECT EXISTS(SELECT 1 FROM flow WHERE workspace_id = $1 AND path = $2) AS "e!""#,
        workspace_id,
        old_path
    )
    .fetch_one(&mut *tx)
    .await?;
    if !source_exists {
        return Err(ApiError::NotFound);
    }

    let occupied: bool = sqlx::query_scalar!(
        r#"SELECT EXISTS(SELECT 1 FROM flow WHERE workspace_id = $1 AND path = $2) AS "e!""#,
        workspace_id,
        new_path
    )
    .fetch_one(&mut *tx)
    .await?;
    if occupied {
        if !req.overwrite {
            return Ok((
                StatusCode::CONFLICT,
                Json(serde_json::json!({ "error": format!("target path '{new_path}' already exists") })),
            )
                .into_response());
        }
        // The target flow is being overwritten: its schedules (by flow_id) go
        // with it.
        sqlx::query!(
            "DELETE FROM schedule WHERE workspace_id = $1
             AND flow_id IN (SELECT flow_id FROM flow WHERE workspace_id = $1 AND path = $2)",
            workspace_id,
            new_path
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query!(
            "DELETE FROM flow WHERE workspace_id = $1 AND path = $2",
            workspace_id,
            new_path
        )
        .execute(&mut *tx)
        .await?;
    }

    // Rename: flow_id is unchanged, so schedules referencing it stay valid with
    // no sync needed.
    sqlx::query!(
        "UPDATE flow SET path = $1 WHERE workspace_id = $2 AND path = $3",
        new_path,
        workspace_id,
        old_path
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok((
        StatusCode::OK,
        Json(serde_json::json!({ "path": new_path })),
    )
        .into_response())
}

#[derive(Deserialize)]
pub struct DeleteFlowRequest {
    pub path: String,
    /// Delete the flow even if schedules reference it (also deletes them).
    #[serde(default)]
    pub force: bool,
}

/// Delete all revisions of a flow. In-flight runs carry their own `flow_value`
/// snapshot, so nothing in-flight blocks. Schedules reference the flow by
/// `flow_id`, so a referenced flow is blocked (409) unless `force` (which also
/// deletes the schedules). Requires write on its folder.
#[tracing::instrument(name = "api::delete_flow", skip(db, user, req), fields(%workspace_id))]
pub async fn delete_flow(
    State(db): State<PgPool>,
    axum::Extension(user): axum::Extension<AuthedUser>,
    Path(workspace_id): Path<String>,
    Json(req): Json<DeleteFlowRequest>,
) -> Result<Response, ApiError> {
    let path = req.path.trim().to_string();
    user.require_writer(&path)?;

    let mut tx = db.begin().await?;
    // Resolve the flow's stable id; schedules reference that, not the path.
    let flow_id = sqlx::query_scalar!(
        "SELECT flow_id FROM flow WHERE workspace_id = $1 AND path = $2
         ORDER BY revision DESC LIMIT 1",
        workspace_id,
        path
    )
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(flow_id) = flow_id {
        let schedules = sqlx::query_scalar!(
            "SELECT name FROM schedule WHERE workspace_id = $1 AND flow_id = $2 ORDER BY name",
            workspace_id,
            flow_id
        )
        .fetch_all(&mut *tx)
        .await?;
        if !schedules.is_empty() && !req.force {
            return Ok((
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error": format!("flow has {} schedule(s)", schedules.len()),
                    "schedules": schedules,
                })),
            )
                .into_response());
        }
        sqlx::query!(
            "DELETE FROM schedule WHERE workspace_id = $1 AND flow_id = $2",
            workspace_id,
            flow_id
        )
        .execute(&mut *tx)
        .await?;
    }
    let res = sqlx::query!(
        "DELETE FROM flow WHERE workspace_id = $1 AND path = $2",
        workspace_id,
        path
    )
    .execute(&mut *tx)
    .await?;
    if res.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    tx.commit().await?;
    Ok((StatusCode::OK, Json(serde_json::json!({ "deleted": true }))).into_response())
}

#[tracing::instrument(name = "api::list_flows", skip(db, _user), fields(%workspace_id))]
pub async fn list_flows(
    State(db): State<PgPool>,
    axum::Extension(_user): axum::Extension<AuthedUser>,
    Path(workspace_id): Path<String>,
) -> Result<Json<Vec<FlowListItem>>, ApiError> {
    // Latest revision per path.
    let rows = sqlx::query!(
        r#"SELECT DISTINCT ON (path)
               flow_id, path, revision, summary AS "summary!", edited_by, edited_at
           FROM flow
           WHERE workspace_id = $1
           ORDER BY path, revision DESC"#,
        workspace_id,
    )
    .fetch_all(&db)
    .await?;

    Ok(Json(
        rows.into_iter()
            .map(|r| FlowListItem {
                flow_id: r.flow_id,
                path: r.path,
                revision: r.revision,
                summary: r.summary,
                edited_by: r.edited_by,
                edited_at: r.edited_at,
            })
            .collect(),
    ))
}

#[tracing::instrument(name = "api::get_flow", skip(db, user), fields(%workspace_id, %path))]
pub async fn get_flow(
    State(db): State<PgPool>,
    axum::Extension(user): axum::Extension<AuthedUser>,
    Path((workspace_id, path)): Path<(String, String)>,
) -> Result<Json<FlowResponse>, ApiError> {
    user.require_reader(&path)?;
    let row = sqlx::query!(
        r#"SELECT flow_id, path, revision, summary AS "summary!", value, edited_by, edited_at
           FROM flow
           WHERE workspace_id = $1 AND path = $2
           ORDER BY revision DESC LIMIT 1"#,
        workspace_id,
        path,
    )
    .fetch_optional(&db)
    .await?
    .ok_or(ApiError::NotFound)?;

    Ok(Json(FlowResponse {
        flow_id: row.flow_id,
        path: row.path,
        revision: row.revision,
        summary: row.summary,
        value: row.value,
        edited_by: row.edited_by,
        edited_at: row.edited_at,
    }))
}

#[tracing::instrument(name = "api::run_flow", skip(db, user, req), fields(%workspace_id, %path))]
pub async fn run_flow(
    State(db): State<PgPool>,
    axum::Extension(user): axum::Extension<AuthedUser>,
    Path((workspace_id, path)): Path<(String, String)>,
    Json(req): Json<RunFlowRequest>,
) -> Result<Response, ApiError> {
    // Running a flow dispatches child runs of the referenced scripts/inline code,
    // bypassing per-run API permission checks. Require writer to match the
    // "writer-to-run" rule for direct script execution (runs.rs); reader-to-run
    // here would let a reader execute scripts they cannot run directly.
    user.require_writer(&path)?;

    let value = sqlx::query_scalar!(
        "SELECT value FROM flow WHERE workspace_id = $1 AND path = $2
         ORDER BY revision DESC LIMIT 1",
        workspace_id,
        path,
    )
    .fetch_optional(&db)
    .await?
    .ok_or(ApiError::NotFound)?;

    let run_id = coveflow_queue::submit_run(
        &db,
        coveflow_queue::NewRun {
            workspace_id: &workspace_id,
            kind: RunKind::Flow,
            script_hash: None,
            script_path: Some(&path),
            raw_code: None,
            language: None,
            args: req.args,
            flow_value: Some(value),
            tag: "default",
            parent_run: None,
            root_run: None,
            flow_step_id: None,
            team_owner: None,
            created_by: &user.email,
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
            schedule_id: None,
            scheduled_time: None,
            data_interval_end: None,
            trigger_id: None,
            trigger_context: None,
        },
    )
    .await?;

    Ok((
        axum::http::StatusCode::CREATED,
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
    use sqlx::PgPool;
    use tower::ServiceExt;

    use crate::test_helpers::*;

    fn simple_flow() -> serde_json::Value {
        serde_json::json!({
            "nodes": [
                { "id": "a", "body": { "kind": "script", "script_id": "11111111-1111-1111-1111-111111111111" } },
                { "id": "b", "body": {
                    "kind": "script", "script_id": "22222222-2222-2222-2222-222222222222",
                    "inputs": { "n": { "kind": "expr", "expr": "steps.a.result.x + 1" } }
                }}
            ],
            "edges": [{ "from": "a", "to": "b" }]
        })
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn create_then_run_flow(pool: PgPool) {
        let user = "dev@test.local";
        seed_account(&pool, user).await;
        seed_workspace_member(&pool, "ws-1", user, "admin").await;
        let token = valid_jwt(user);
        let app = crate::create_router(
            pool.clone(),
            test_metrics(),
            crate::test_helpers::test_secret_key(),
        );

        // Create.
        let body = serde_json::json!({ "path": "workspace/test/myflow", "value": simple_flow() });
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/workspaces/ws-1/flows/create")
                    .header("Authorization", format!("Bearer {token}"))
                    .header("Content-Type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        // Trigger.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/workspaces/ws-1/flows/run/workspace/test/myflow")
                    .header("Authorization", format!("Bearer {token}"))
                    .header("Content-Type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let run_id = json["id"].as_str().unwrap();

        // The triggered run is a flow run carrying the definition.
        let row = sqlx::query!(
            "SELECT kind, flow_value IS NOT NULL AS \"has_value!\" FROM run WHERE id = $1::uuid",
            run_id as &str
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.kind, "flow");
        assert!(row.has_value);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn rejects_bad_expression(pool: PgPool) {
        let user = "dev@test.local";
        seed_account(&pool, user).await;
        seed_workspace_member(&pool, "ws-1", user, "admin").await;
        let token = valid_jwt(user);
        let app =
            crate::create_router(pool, test_metrics(), crate::test_helpers::test_secret_key());

        let bad = serde_json::json!({
            "path": "workspace/test/bad",
            "value": { "nodes": [
                { "id": "a", "body": {
                    "kind": "script", "script_id": "33333333-3333-3333-3333-333333333333",
                    "inputs": { "x": { "kind": "expr", "expr": "foo(" } }
                }}
            ]}
        });
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/workspaces/ws-1/flows/create")
                    .header("Authorization", format!("Bearer {token}"))
                    .header("Content-Type", "application/json")
                    .body(Body::from(bad.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // --- move / delete -----------------------------------------------------

    async fn post(
        app: &axum::Router,
        token: &str,
        uri: &str,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header("Authorization", format!("Bearer {token}"))
                    .header("Content-Type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, json)
    }
    async fn get_status(app: &axum::Router, token: &str, uri: &str) -> StatusCode {
        app.clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(uri)
                    .header("Authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
    }
    async fn get_json(
        app: &axum::Router,
        token: &str,
        uri: &str,
    ) -> (StatusCode, serde_json::Value) {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(uri)
                    .header("Authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, json)
    }
    async fn create_flow_at(app: &axum::Router, token: &str, path: &str) -> uuid::Uuid {
        let (s, body) = post(
            app,
            token,
            "/api/workspaces/ws-1/flows/create",
            serde_json::json!({ "path": path, "value": simple_flow() }),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED);
        body["flow_id"]
            .as_str()
            .expect("create response has flow_id")
            .parse()
            .unwrap()
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn create_rejects_non_root_path(pool: PgPool) {
        let user = "dev@test.local";
        seed_account(&pool, user).await;
        seed_workspace_member(&pool, "ws-1", user, "admin").await;
        let token = valid_jwt(user);
        let app =
            crate::create_router(pool, test_metrics(), crate::test_helpers::test_secret_key());
        // Even an admin can't create outside the three roots (no 4th top-level folder).
        let (s, _) = post(
            &app,
            &token,
            "/api/workspaces/ws-1/flows/create",
            serde_json::json!({ "path": "f/legacy/x", "value": simple_flow() }),
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn move_flow_changes_path(pool: PgPool) {
        let user = "dev@test.local";
        seed_account(&pool, user).await;
        seed_workspace_member(&pool, "ws-1", user, "admin").await;
        let token = valid_jwt(user);
        let app =
            crate::create_router(pool, test_metrics(), crate::test_helpers::test_secret_key());

        create_flow_at(&app, &token, "workspace/a/x").await;
        let (s, _) = post(
            &app,
            &token,
            "/api/workspaces/ws-1/flows/move",
            serde_json::json!({ "old_path": "workspace/a/x", "new_path": "workspace/b/y" }),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(
            get_status(&app, &token, "/api/workspaces/ws-1/flows/get/workspace/a/x").await,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            get_status(&app, &token, "/api/workspaces/ws-1/flows/get/workspace/b/y").await,
            StatusCode::OK
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn move_flow_collision_rejected_then_overwrites(pool: PgPool) {
        let user = "dev@test.local";
        seed_account(&pool, user).await;
        seed_workspace_member(&pool, "ws-1", user, "admin").await;
        let token = valid_jwt(user);
        let app =
            crate::create_router(pool, test_metrics(), crate::test_helpers::test_secret_key());

        create_flow_at(&app, &token, "workspace/a").await;
        create_flow_at(&app, &token, "workspace/b").await;
        // Collision without overwrite → 409.
        let (s, _) = post(
            &app,
            &token,
            "/api/workspaces/ws-1/flows/move",
            serde_json::json!({ "old_path": "workspace/a", "new_path": "workspace/b" }),
        )
        .await;
        assert_eq!(s, StatusCode::CONFLICT);
        // With overwrite → 200, source gone, target taken.
        let (s, _) = post(
            &app,
            &token,
            "/api/workspaces/ws-1/flows/move",
            serde_json::json!({ "old_path": "workspace/a", "new_path": "workspace/b", "overwrite": true }),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(
            get_status(&app, &token, "/api/workspaces/ws-1/flows/get/workspace/a").await,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            get_status(&app, &token, "/api/workspaces/ws-1/flows/get/workspace/b").await,
            StatusCode::OK
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn delete_flow_removes_all_revisions(pool: PgPool) {
        let user = "dev@test.local";
        seed_account(&pool, user).await;
        seed_workspace_member(&pool, "ws-1", user, "admin").await;
        let token = valid_jwt(user);
        let app =
            crate::create_router(pool, test_metrics(), crate::test_helpers::test_secret_key());

        create_flow_at(&app, &token, "workspace/c").await;
        create_flow_at(&app, &token, "workspace/c").await; // second revision
        let (s, _) = post(
            &app,
            &token,
            "/api/workspaces/ws-1/flows/delete",
            serde_json::json!({ "path": "workspace/c" }),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(
            get_status(&app, &token, "/api/workspaces/ws-1/flows/get/workspace/c").await,
            StatusCode::NOT_FOUND
        );
        // Deleting a non-existent flow → 404.
        let (s, _) = post(
            &app,
            &token,
            "/api/workspaces/ws-1/flows/delete",
            serde_json::json!({ "path": "workspace/nope" }),
        )
        .await;
        assert_eq!(s, StatusCode::NOT_FOUND);
    }

    // --- schedule integrity (R5) -------------------------------------------

    #[sqlx::test(migrations = "../../migrations")]
    async fn move_flow_rewrites_schedule_path(pool: PgPool) {
        let user = "dev@test.local";
        seed_account(&pool, user).await;
        seed_workspace_member(&pool, "ws-1", user, "admin").await;
        let token = valid_jwt(user);
        let app =
            crate::create_router(pool, test_metrics(), crate::test_helpers::test_secret_key());

        let fid = create_flow_at(&app, &token, "workspace/a/x").await;
        // A schedule references the flow by stable id.
        let (s, body) = post(
            &app,
            &token,
            "/api/workspaces/ws-1/schedules/create",
            serde_json::json!({ "name": "nightly", "flow_id": fid, "cron_expr": "0 2 * * *" }),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED);
        let sid = body["id"].as_str().unwrap().to_string();

        // Renaming the flow leaves the schedule untouched (it references flow_id);
        // get_schedule still resolves to the new current path.
        let (s, _) = post(
            &app,
            &token,
            "/api/workspaces/ws-1/flows/move",
            serde_json::json!({ "old_path": "workspace/a/x", "new_path": "workspace/b/y" }),
        )
        .await;
        assert_eq!(s, StatusCode::OK);

        let (s, sched) = get_json(
            &app,
            &token,
            &format!("/api/workspaces/ws-1/schedules/get/{sid}"),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(sched["flow_id"], fid.to_string());
        assert_eq!(sched["flow_path"], "workspace/b/y");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn delete_flow_blocked_by_schedule_unless_force(pool: PgPool) {
        let user = "dev@test.local";
        seed_account(&pool, user).await;
        seed_workspace_member(&pool, "ws-1", user, "admin").await;
        let token = valid_jwt(user);
        let app =
            crate::create_router(pool, test_metrics(), crate::test_helpers::test_secret_key());

        let fid = create_flow_at(&app, &token, "workspace/c").await;
        let (s, body) = post(
            &app,
            &token,
            "/api/workspaces/ws-1/schedules/create",
            serde_json::json!({ "name": "nightly", "flow_id": fid, "cron_expr": "0 2 * * *" }),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED);
        let sid = body["id"].as_str().unwrap().to_string();

        // Referenced by a schedule → delete blocked (409), flow still present.
        let (s, _) = post(
            &app,
            &token,
            "/api/workspaces/ws-1/flows/delete",
            serde_json::json!({ "path": "workspace/c" }),
        )
        .await;
        assert_eq!(s, StatusCode::CONFLICT);
        assert_eq!(
            get_status(&app, &token, "/api/workspaces/ws-1/flows/get/workspace/c").await,
            StatusCode::OK
        );

        // force → flow gone AND the referencing schedule deleted with it.
        let (s, _) = post(
            &app,
            &token,
            "/api/workspaces/ws-1/flows/delete",
            serde_json::json!({ "path": "workspace/c", "force": true }),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(
            get_status(&app, &token, "/api/workspaces/ws-1/flows/get/workspace/c").await,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            get_status(
                &app,
                &token,
                &format!("/api/workspaces/ws-1/schedules/get/{sid}")
            )
            .await,
            StatusCode::NOT_FOUND
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn move_overwrite_cascades_target_schedules(pool: PgPool) {
        let user = "dev@test.local";
        seed_account(&pool, user).await;
        seed_workspace_member(&pool, "ws-1", user, "admin").await;
        let token = valid_jwt(user);
        let app =
            crate::create_router(pool, test_metrics(), crate::test_helpers::test_secret_key());

        create_flow_at(&app, &token, "workspace/src").await;
        let target_fid = create_flow_at(&app, &token, "workspace/dst").await;
        // A schedule on the target flow that is about to be overwritten.
        let (s, body) = post(
            &app,
            &token,
            "/api/workspaces/ws-1/schedules/create",
            serde_json::json!({ "name": "nightly", "flow_id": target_fid, "cron_expr": "0 2 * * *" }),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED);
        let sid = body["id"].as_str().unwrap().to_string();

        // Overwrite move: src replaces dst → dst's schedules go with it.
        let (s, _) = post(
            &app,
            &token,
            "/api/workspaces/ws-1/flows/move",
            serde_json::json!({ "old_path": "workspace/src", "new_path": "workspace/dst", "overwrite": true }),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(
            get_status(
                &app,
                &token,
                &format!("/api/workspaces/ws-1/schedules/get/{sid}")
            )
            .await,
            StatusCode::NOT_FOUND,
            "the replaced target flow's schedule is cascade-deleted"
        );
    }
}
