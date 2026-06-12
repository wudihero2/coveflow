//! Trigger CRUD. Workspace-scoped, JWT-authed. Managing a trigger
//! needs `can_write` on its flow; listing needs `can_read`. v1 ships the
//! `webhook` type; the inbound endpoint lives in [`crate::webhooks`].

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use coveflow_queue::{TriggerError, TriggerKind, WebhookTrigger};
use coveflow_types::trigger::{TriggerRow, WEBHOOK_TYPE};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::AuthedUser;
use crate::error::ApiError;
use crate::schedules::current_flow_path;

#[derive(Serialize)]
pub struct TriggerResponse {
    pub id: Uuid,
    pub flow_id: Uuid,
    pub trigger_type: String,
    pub name: String,
    pub enabled: bool,
    pub config: Value,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Path the external caller POSTs to (the frontend prepends its origin).
    pub webhook_path: String,
}

#[allow(clippy::too_many_arguments)]
fn to_response(
    id: Uuid,
    flow_id: Uuid,
    trigger_type: String,
    name: String,
    enabled: bool,
    config: Value,
    created_by: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
) -> TriggerResponse {
    TriggerResponse {
        webhook_path: format!("/api/webhooks/{id}"),
        id,
        flow_id,
        trigger_type,
        name,
        enabled,
        config,
        created_by,
        created_at,
        updated_at,
    }
}

#[derive(Deserialize)]
pub struct CreateTriggerRequest {
    #[serde(default = "default_type")]
    pub trigger_type: String,
    pub name: String,
    #[serde(default)]
    pub config: Option<Value>,
}

fn default_type() -> String {
    WEBHOOK_TYPE.to_string()
}

#[derive(Deserialize)]
pub struct UpdateTriggerRequest {
    pub name: Option<String>,
    pub enabled: Option<bool>,
    pub config: Option<Value>,
}

/// Validate a type-specific config (v1: only webhook).
fn validate(trigger_type: &str, config: &Value) -> Result<(), ApiError> {
    match trigger_type {
        WEBHOOK_TYPE => WebhookTrigger::validate_config(config).map_err(|e| match e {
            TriggerError::InvalidConfig(m) => ApiError::BadRequest(m),
            other => ApiError::Internal(other.to_string()),
        }),
        other => Err(ApiError::BadRequest(format!(
            "unsupported trigger type '{other}'"
        ))),
    }
}

#[tracing::instrument(name = "api::list_triggers", skip(db, user), fields(%workspace_id, %flow_id))]
pub async fn list_triggers(
    State(db): State<PgPool>,
    axum::Extension(user): axum::Extension<AuthedUser>,
    Path((workspace_id, flow_id)): Path<(String, Uuid)>,
) -> Result<Json<Vec<TriggerResponse>>, ApiError> {
    let path = current_flow_path(&db, &workspace_id, flow_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    user.require_reader(&path)?;

    let rows = sqlx::query!(
        "SELECT id, flow_id, trigger_type, name, enabled, config, created_by, created_at, updated_at
         FROM trigger WHERE workspace_id = $1 AND flow_id = $2 ORDER BY name",
        workspace_id,
        flow_id
    )
    .fetch_all(&db)
    .await?;

    let items = rows
        .into_iter()
        .map(|r| {
            to_response(
                r.id,
                r.flow_id,
                r.trigger_type,
                r.name,
                r.enabled,
                r.config,
                r.created_by,
                r.created_at,
                r.updated_at,
            )
        })
        .collect();
    Ok(Json(items))
}

#[tracing::instrument(name = "api::create_trigger", skip(db, user, req), fields(%workspace_id, %flow_id, name = %req.name))]
pub async fn create_trigger(
    State(db): State<PgPool>,
    axum::Extension(user): axum::Extension<AuthedUser>,
    Path((workspace_id, flow_id)): Path<(String, Uuid)>,
    Json(req): Json<CreateTriggerRequest>,
) -> Result<Response, ApiError> {
    let path = current_flow_path(&db, &workspace_id, flow_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    user.require_writer(&path)?;
    if req.name.trim().is_empty() {
        return Err(ApiError::BadRequest("name must not be empty".into()));
    }
    let config = req.config.unwrap_or_else(|| json!({}));
    validate(&req.trigger_type, &config)?;

    let row = sqlx::query!(
        "INSERT INTO trigger (workspace_id, flow_id, trigger_type, name, config, created_by)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (workspace_id, flow_id, name) DO NOTHING
         RETURNING id, enabled, created_at, updated_at",
        workspace_id,
        flow_id,
        req.trigger_type,
        req.name,
        config,
        user.email,
    )
    .fetch_optional(&db)
    .await?
    .ok_or_else(|| ApiError::Conflict(format!("trigger '{}' already exists", req.name)))?;

    Ok((
        StatusCode::CREATED,
        Json(to_response(
            row.id,
            flow_id,
            req.trigger_type,
            req.name,
            row.enabled,
            config,
            user.email,
            row.created_at,
            row.updated_at,
        )),
    )
        .into_response())
}

/// Load a trigger by id (scoped to workspace), 404 if missing.
async fn load_trigger(db: &PgPool, workspace_id: &str, id: Uuid) -> Result<TriggerRow, ApiError> {
    sqlx::query!(
        "SELECT id, workspace_id, flow_id, trigger_type, name, enabled, config, created_by
         FROM trigger WHERE workspace_id = $1 AND id = $2",
        workspace_id,
        id
    )
    .fetch_optional(db)
    .await?
    .map(|r| TriggerRow {
        id: r.id,
        workspace_id: r.workspace_id,
        flow_id: r.flow_id,
        trigger_type: r.trigger_type,
        name: r.name,
        enabled: r.enabled,
        config: r.config,
        created_by: r.created_by,
    })
    .ok_or(ApiError::NotFound)
}

#[tracing::instrument(name = "api::update_trigger", skip(db, user, req), fields(%workspace_id, %id))]
pub async fn update_trigger(
    State(db): State<PgPool>,
    axum::Extension(user): axum::Extension<AuthedUser>,
    Path((workspace_id, id)): Path<(String, Uuid)>,
    Json(req): Json<UpdateTriggerRequest>,
) -> Result<Response, ApiError> {
    let trigger = load_trigger(&db, &workspace_id, id).await?;
    let path = current_flow_path(&db, &workspace_id, trigger.flow_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    user.require_writer(&path)?;

    if let Some(config) = &req.config {
        validate(&trigger.trigger_type, config)?;
    }

    sqlx::query!(
        "UPDATE trigger
         SET name = COALESCE($3, name),
             enabled = COALESCE($4, enabled),
             config = COALESCE($5, config),
             updated_at = now()
         WHERE workspace_id = $1 AND id = $2",
        workspace_id,
        id,
        req.name,
        req.enabled,
        req.config,
    )
    .execute(&db)
    .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[tracing::instrument(name = "api::delete_trigger", skip(db, user), fields(%workspace_id, %id))]
pub async fn delete_trigger(
    State(db): State<PgPool>,
    axum::Extension(user): axum::Extension<AuthedUser>,
    Path((workspace_id, id)): Path<(String, Uuid)>,
) -> Result<Response, ApiError> {
    let trigger = load_trigger(&db, &workspace_id, id).await?;
    let path = current_flow_path(&db, &workspace_id, trigger.flow_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    user.require_writer(&path)?;

    sqlx::query!(
        "DELETE FROM trigger WHERE workspace_id = $1 AND id = $2",
        workspace_id,
        id
    )
    .execute(&db)
    .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[cfg(test)]
mod tests {
    use crate::test_helpers::{call_json, seed_account, seed_flow, seed_workspace_member};
    use axum::http::StatusCode;
    use sqlx::PgPool;

    async fn member(db: &PgPool, ws: &str, email: &str, role: &str) {
        seed_account(db, email).await;
        seed_workspace_member(db, ws, email, role).await;
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn create_list_and_acl(pool: PgPool) {
        member(&pool, "ws1", "a@x.com", "admin").await;
        member(&pool, "ws1", "v@x.com", "viewer").await;
        let flow_id = seed_flow(&pool, "ws1", "workspace/f").await;
        let base = format!("/api/workspaces/ws1/flows/{flow_id}/triggers");

        // Writer creates a webhook.
        let (st, body) = call_json(
            pool.clone(),
            "POST",
            &base,
            "a@x.com",
            Some(serde_json::json!({ "name": "hook", "config": { "max_active_runs": 2 } })),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED);
        assert_eq!(body["trigger_type"], "webhook");
        let id = body["id"].as_str().unwrap();
        assert_eq!(body["webhook_path"], format!("/api/webhooks/{id}"));

        // Reader can list.
        let (st, list) = call_json(pool.clone(), "GET", &base, "v@x.com", None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(list.as_array().unwrap().len(), 1);

        // Viewer cannot create (no can_write).
        let (st, _) = call_json(
            pool.clone(),
            "POST",
            &base,
            "v@x.com",
            Some(serde_json::json!({ "name": "x" })),
        )
        .await;
        assert_eq!(st, StatusCode::FORBIDDEN);

        // Duplicate name → 409.
        let (st, _) = call_json(
            pool.clone(),
            "POST",
            &base,
            "a@x.com",
            Some(serde_json::json!({ "name": "hook" })),
        )
        .await;
        assert_eq!(st, StatusCode::CONFLICT);

        // Update enabled, then delete.
        let (st, _) = call_json(
            pool.clone(),
            "PUT",
            &format!("/api/workspaces/ws1/triggers/{id}"),
            "a@x.com",
            Some(serde_json::json!({ "enabled": false })),
        )
        .await;
        assert_eq!(st, StatusCode::NO_CONTENT);
        let (st, _) = call_json(
            pool.clone(),
            "DELETE",
            &format!("/api/workspaces/ws1/triggers/{id}"),
            "a@x.com",
            None,
        )
        .await;
        assert_eq!(st, StatusCode::NO_CONTENT);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn invalid_config_is_400(pool: PgPool) {
        member(&pool, "ws1", "a@x.com", "admin").await;
        let flow_id = seed_flow(&pool, "ws1", "workspace/f").await;
        let (st, _) = call_json(
            pool.clone(),
            "POST",
            &format!("/api/workspaces/ws1/flows/{flow_id}/triggers"),
            "a@x.com",
            Some(serde_json::json!({ "name": "h", "config": { "max_active_runs": 0 } })),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
    }
}
