//! Secret store CRUD.
//!
//! Workspace-scoped, path-addressed, **write-only** encrypted key-value. Values
//! are AES-256-GCM encrypted on write ([`coveflow_types::crypto`]) and never read
//! back through the API — only the worker decrypts them to inject into a run.
//! Managing a secret requires write on its three-root path; listing returns only
//! metadata for the paths the caller can read.
//!
//! SECURITY: plaintext values never appear in responses, logs, or trace fields.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use coveflow_types::crypto::{self, SecretKey};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::auth::AuthedUser;
use crate::error::ApiError;

/// Secret metadata — never includes the value (write-only store).
#[derive(Serialize)]
pub struct SecretListItem {
    pub path: String,
    pub description: String,
    pub created_by: String,
    pub updated_by: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Deserialize)]
pub struct CreateSecretRequest {
    pub path: String,
    pub value: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Deserialize)]
pub struct RotateSecretRequest {
    pub value: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// Structural check: the path sits under a three-system root with a non-empty
/// sub-path. Ownership/role is a separate `require_writer` check, so an invalid
/// shape returns 400 while a valid-but-unauthorized path returns 403.
fn is_valid_secret_path(path: &str) -> bool {
    if let Some(rest) = path.strip_prefix("workspace/") {
        return !rest.is_empty();
    }
    for root in ["users/", "teams/"] {
        if let Some(rest) = path.strip_prefix(root) {
            return rest
                .split_once('/')
                .is_some_and(|(name, sub)| !name.is_empty() && !sub.is_empty());
        }
    }
    false
}

#[tracing::instrument(name = "api::list_secrets", skip(db, user), fields(%workspace_id))]
pub async fn list_secrets(
    State(db): State<PgPool>,
    axum::Extension(user): axum::Extension<AuthedUser>,
    Path(workspace_id): Path<String>,
) -> Result<Json<Vec<SecretListItem>>, ApiError> {
    let rows = sqlx::query!(
        "SELECT path, description, created_by, updated_by, updated_at
         FROM secret WHERE workspace_id = $1 ORDER BY path",
        workspace_id
    )
    .fetch_all(&db)
    .await?;

    // Same as runs/schedules: list endpoints are unfiltered in SQL, ACL applied
    // in process (membership isn't expressible in the query).
    let items = rows
        .into_iter()
        .filter(|r| user.can_read(&r.path))
        .map(|r| SecretListItem {
            path: r.path,
            description: r.description,
            created_by: r.created_by,
            updated_by: r.updated_by,
            updated_at: r.updated_at,
        })
        .collect();
    Ok(Json(items))
}

#[tracing::instrument(name = "api::create_secret", skip(db, key, user, req), fields(%workspace_id, path = %req.path))]
pub async fn create_secret(
    State(db): State<PgPool>,
    State(key): State<SecretKey>,
    axum::Extension(user): axum::Extension<AuthedUser>,
    Path(workspace_id): Path<String>,
    Json(req): Json<CreateSecretRequest>,
) -> Result<Response, ApiError> {
    if !is_valid_secret_path(&req.path) {
        return Err(ApiError::BadRequest(
            "path must be under users/<you>/, teams/<your team>/, or workspace/".into(),
        ));
    }
    if req.value.is_empty() {
        return Err(ApiError::BadRequest("value must not be empty".into()));
    }
    user.require_writer(&req.path)?;

    let blob = crypto::encrypt(&key, req.value.as_bytes())
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let description = req.description.unwrap_or_default();

    // ON CONFLICT DO NOTHING + RETURNING: a None result means the path already
    // exists → 409 (rotate it instead of recreating).
    let inserted = sqlx::query_scalar!(
        "INSERT INTO secret (workspace_id, path, value_encrypted, description, created_by, updated_by)
         VALUES ($1, $2, $3, $4, $5, $5)
         ON CONFLICT (workspace_id, path) DO NOTHING
         RETURNING path",
        workspace_id,
        req.path,
        blob,
        description,
        user.email,
    )
    .fetch_optional(&db)
    .await?;

    if inserted.is_none() {
        return Err(ApiError::Conflict(format!(
            "secret '{}' already exists",
            req.path
        )));
    }
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({ "path": req.path })),
    )
        .into_response())
}

#[tracing::instrument(name = "api::rotate_secret", skip(db, key, user, req), fields(%workspace_id, %path))]
pub async fn rotate_secret(
    State(db): State<PgPool>,
    State(key): State<SecretKey>,
    axum::Extension(user): axum::Extension<AuthedUser>,
    Path((workspace_id, path)): Path<(String, String)>,
    Json(req): Json<RotateSecretRequest>,
) -> Result<Response, ApiError> {
    if req.value.is_empty() {
        return Err(ApiError::BadRequest("value must not be empty".into()));
    }
    user.require_writer(&path)?;

    let blob = crypto::encrypt(&key, req.value.as_bytes())
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    // COALESCE keeps the existing description when the request omits it.
    let updated = sqlx::query_scalar!(
        "UPDATE secret
         SET value_encrypted = $3,
             description = COALESCE($4, description),
             updated_by = $5,
             updated_at = now()
         WHERE workspace_id = $1 AND path = $2
         RETURNING path",
        workspace_id,
        path,
        blob,
        req.description,
        user.email,
    )
    .fetch_optional(&db)
    .await?;

    updated.ok_or(ApiError::NotFound)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[tracing::instrument(name = "api::delete_secret", skip(db, user), fields(%workspace_id, %path))]
pub async fn delete_secret(
    State(db): State<PgPool>,
    axum::Extension(user): axum::Extension<AuthedUser>,
    Path((workspace_id, path)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    user.require_writer(&path)?;

    let res = sqlx::query!(
        "DELETE FROM secret WHERE workspace_id = $1 AND path = $2",
        workspace_id,
        path
    )
    .execute(&db)
    .await?;

    if res.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[cfg(test)]
mod tests {
    use crate::test_helpers::{call_json, seed_account, seed_workspace_member};
    use axum::http::StatusCode;
    use sqlx::PgPool;

    async fn member(db: &PgPool, ws: &str, email: &str, role: &str) {
        seed_account(db, email).await;
        seed_workspace_member(db, ws, email, role).await;
    }

    fn create_body(path: &str, value: &str) -> serde_json::Value {
        serde_json::json!({ "path": path, "value": value, "description": "desc" })
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn create_then_list_hides_value(pool: PgPool) {
        member(&pool, "ws1", "a@x.com", "admin").await;

        let (st, _) = call_json(
            pool.clone(),
            "POST",
            "/api/workspaces/ws1/secrets",
            "a@x.com",
            Some(create_body("workspace/openai", "sk-123")),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED);

        // Stored value is encrypted, not the plaintext.
        let blob: Vec<u8> = sqlx::query_scalar!(
            "SELECT value_encrypted FROM secret WHERE workspace_id = 'ws1' AND path = 'workspace/openai'"
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(!blob.windows(6).any(|w| w == b"sk-123"));

        let (st, body) = call_json(
            pool.clone(),
            "GET",
            "/api/workspaces/ws1/secrets",
            "a@x.com",
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let arr = body.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["path"], "workspace/openai");
        assert!(arr[0].get("value").is_none(), "list must not expose value");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn non_root_path_is_400(pool: PgPool) {
        member(&pool, "ws1", "a@x.com", "admin").await;
        let (st, _) = call_json(
            pool.clone(),
            "POST",
            "/api/workspaces/ws1/secrets",
            "a@x.com",
            Some(create_body("foo/bar", "v")),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn empty_value_is_400(pool: PgPool) {
        member(&pool, "ws1", "a@x.com", "admin").await;
        let (st, _) = call_json(
            pool.clone(),
            "POST",
            "/api/workspaces/ws1/secrets",
            "a@x.com",
            Some(create_body("workspace/k", "")),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn other_users_path_is_403(pool: PgPool) {
        // Non-admin role: the workspace Admin role bypasses can_write entirely.
        member(&pool, "ws1", "a@x.com", "editor").await;
        let (st, _) = call_json(
            pool.clone(),
            "POST",
            "/api/workspaces/ws1/secrets",
            "a@x.com",
            Some(create_body("users/b@x.com/k", "v")),
        )
        .await;
        assert_eq!(st, StatusCode::FORBIDDEN);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn duplicate_create_is_409(pool: PgPool) {
        member(&pool, "ws1", "a@x.com", "admin").await;
        let body = Some(create_body("workspace/dup", "v1"));
        let (st, _) = call_json(
            pool.clone(),
            "POST",
            "/api/workspaces/ws1/secrets",
            "a@x.com",
            body.clone(),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED);
        let (st, _) = call_json(
            pool.clone(),
            "POST",
            "/api/workspaces/ws1/secrets",
            "a@x.com",
            body,
        )
        .await;
        assert_eq!(st, StatusCode::CONFLICT);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn rotate_updates_metadata_and_404s_when_missing(pool: PgPool) {
        member(&pool, "ws1", "a@x.com", "admin").await;
        call_json(
            pool.clone(),
            "POST",
            "/api/workspaces/ws1/secrets",
            "a@x.com",
            Some(create_body("workspace/r", "v1")),
        )
        .await;

        let (st, _) = call_json(
            pool.clone(),
            "PUT",
            "/api/workspaces/ws1/secrets/workspace/r",
            "a@x.com",
            Some(serde_json::json!({ "value": "v2" })),
        )
        .await;
        assert_eq!(st, StatusCode::NO_CONTENT);

        // Missing path → 404.
        let (st, _) = call_json(
            pool.clone(),
            "PUT",
            "/api/workspaces/ws1/secrets/workspace/missing",
            "a@x.com",
            Some(serde_json::json!({ "value": "v" })),
        )
        .await;
        assert_eq!(st, StatusCode::NOT_FOUND);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn list_excludes_unreadable_user_secrets(pool: PgPool) {
        // a@x.com is a plain member; b@x.com's user-root secret must not appear.
        member(&pool, "ws1", "a@x.com", "editor").await;
        sqlx::query!(
            "INSERT INTO secret (workspace_id, path, value_encrypted, created_by, updated_by)
             VALUES ('ws1', 'users/b@x.com/k', $1, 'b@x.com', 'b@x.com')",
            vec![0u8; 16]
        )
        .execute(&pool)
        .await
        .unwrap();

        let (st, body) = call_json(
            pool.clone(),
            "GET",
            "/api/workspaces/ws1/secrets",
            "a@x.com",
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(
            body.as_array().unwrap().len(),
            0,
            "other user's secret hidden"
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn delete_removes_and_404s_when_missing(pool: PgPool) {
        member(&pool, "ws1", "a@x.com", "admin").await;
        call_json(
            pool.clone(),
            "POST",
            "/api/workspaces/ws1/secrets",
            "a@x.com",
            Some(create_body("workspace/d", "v")),
        )
        .await;
        let (st, _) = call_json(
            pool.clone(),
            "DELETE",
            "/api/workspaces/ws1/secrets/workspace/d",
            "a@x.com",
            None,
        )
        .await;
        assert_eq!(st, StatusCode::NO_CONTENT);
        let (st, _) = call_json(
            pool.clone(),
            "DELETE",
            "/api/workspaces/ws1/secrets/workspace/d",
            "a@x.com",
            None,
        )
        .await;
        assert_eq!(st, StatusCode::NOT_FOUND);
    }
}
