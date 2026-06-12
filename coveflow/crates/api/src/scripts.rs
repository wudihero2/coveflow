use axum::Extension;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};
use sqlx::PgPool;

use coveflow_types::ScriptLang;
use coveflow_types::scripts::{is_valid_runtime, normalize_requirements};

use crate::auth::AuthedUser;
use crate::error::ApiError;
use crate::script_schema::{MainSchema, extract_main_schema};

#[derive(serde::Deserialize)]
pub struct CreateScriptRequest {
    pub path: String,
    /// Human-readable display name (shown in lists and flow nodes). Required.
    pub name: String,
    pub content: String,
    pub language: ScriptLang,
    pub summary: Option<String>,
    pub requirements: Option<Vec<String>>,
    /// Container image tag for execution, e.g. "python:3.12".
    /// NULL means use platform default for the language.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
}

#[derive(serde::Serialize)]
pub struct ScriptCreated {
    pub hash: String,
}

#[derive(serde::Serialize)]
pub struct ScriptResponse {
    pub workspace_id: String,
    pub hash: String,
    pub script_id: uuid::Uuid,
    pub path: String,
    pub name: String,
    pub content: String,
    pub language: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
    pub schema: Option<serde_json::Value>,
    pub parent_hashes: Option<Vec<String>>,
    pub summary: String,
    pub requirements: Vec<String>,
    pub created_by: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(serde::Deserialize)]
pub struct ListScriptsQuery {
    pub path_prefix: Option<String>,
}

#[derive(serde::Serialize)]
pub struct ScriptListItem {
    pub hash: String,
    pub script_id: uuid::Uuid,
    pub path: String,
    pub name: String,
    pub language: String,
    pub summary: String,
    pub created_by: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(serde::Deserialize)]
pub struct ScriptVersionsQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(serde::Serialize)]
pub struct ScriptVersionItem {
    pub hash: String,
    pub summary: String,
    pub created_by: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub parent_hashes: Option<Vec<String>>,
}

#[derive(serde::Serialize)]
pub struct ScriptVersionsResponse {
    pub items: Vec<ScriptVersionItem>,
    pub total: i64,
    pub has_more: bool,
}

fn script_versions_page(query: &ScriptVersionsQuery) -> Result<(i64, i64), ApiError> {
    const DEFAULT_LIMIT: i64 = 20;
    const MAX_LIMIT: i64 = 100;

    let limit = query.limit.unwrap_or(DEFAULT_LIMIT);
    if limit <= 0 {
        return Err(ApiError::BadRequest("limit must be greater than 0".into()));
    }

    let offset = query.offset.unwrap_or(0);
    if offset < 0 {
        return Err(ApiError::BadRequest(
            "offset must be greater than or equal to 0".into(),
        ));
    }

    Ok((limit.min(MAX_LIMIT), offset))
}

fn compute_script_hash(
    content: &str,
    path: &str,
    language: &str,
    runtime: Option<&str>,
    requirements: &[String],
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    hasher.update(b"\0");
    hasher.update(path.as_bytes());
    hasher.update(b"\0");
    hasher.update(language.as_bytes());
    hasher.update(b"\0");
    if let Some(rt) = runtime {
        hasher.update(rt.as_bytes());
    }
    hasher.update(b"\0");
    for req in requirements {
        hasher.update(req.as_bytes());
        hasher.update(b"\n");
    }
    format!("{:x}", hasher.finalize())
}

fn validate_script_runtime_and_requirements(
    _language: &ScriptLang,
    runtime: Option<&str>,
    _requirements: &[String],
) -> Result<(), ApiError> {
    if !is_valid_runtime(runtime) {
        return Err(ApiError::BadRequest(format!(
            "invalid runtime: {runtime:?}"
        )));
    }

    Ok(())
}

/// Transaction-scoped advisory lock keyed on (workspace, path). Makes the
/// "is this path occupied?" check and the following write atomic against
/// concurrent create/move on the same path (there is no DB unique constraint on
/// path — the versioned model allows multiple rows per path).
async fn lock_path(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: &str,
    path: &str,
) -> Result<(), ApiError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1 || ':' || $2)::bigint)")
        .bind(workspace_id)
        .bind(path)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

#[tracing::instrument(name = "api::create_script", skip(db, user, req), fields(%workspace_id))]
pub async fn create_script(
    State(db): State<PgPool>,
    Extension(user): Extension<AuthedUser>,
    Path(workspace_id): Path<String>,
    Json(req): Json<CreateScriptRequest>,
) -> Result<Response, ApiError> {
    if req.path.is_empty() {
        return Err(ApiError::BadRequest("path must not be empty".into()));
    }
    if req.path.ends_with('/') {
        return Err(ApiError::BadRequest(
            "script name is required (the path must end with a name, not '/')".into(),
        ));
    }
    if !user.is_valid_root_path(&req.path) {
        return Err(ApiError::BadRequest(
            "path must be under users/<you>/, teams/<your team>/, or workspace/".into(),
        ));
    }
    user.require_writer(&req.path)?;

    if req.content.is_empty() {
        return Err(ApiError::BadRequest("content must not be empty".into()));
    }

    let lang_str = req.language.as_str();
    // Filesystem model: the display name is always the path's leaf (no separate
    // drifting label), so a move/rename (path change) updates the name with it.
    let name = req.path.rsplit('/').next().unwrap_or(&req.path).to_string();
    let summary = req.summary.unwrap_or_default();
    let requirements = normalize_requirements(req.requirements);
    validate_script_runtime_and_requirements(&req.language, req.runtime.as_deref(), &requirements)?;

    // Auto-detect the main() signature into a JSON Schema for flow input prefill.
    // A parseable script with no top-level main() can never be executed
    // (the worker calls mod.main(**args)), so reject it early. If our parser
    // can't handle the source, store no schema rather than blocking the save.
    let schema: Option<serde_json::Value> = match req.language {
        ScriptLang::Python3 => match extract_main_schema(&req.content) {
            MainSchema::Found(v) => Some(v),
            MainSchema::NoMain => {
                return Err(ApiError::BadRequest(
                    "script must define a top-level main() function: it is the entrypoint the \
                     worker calls (main(**args)). Library-only modules without a main() cannot \
                     be saved or run."
                        .into(),
                ));
            }
            MainSchema::Unparseable => None,
        },
    };
    let hash = compute_script_hash(
        &req.content,
        &req.path,
        lang_str,
        req.runtime.as_deref(),
        &requirements,
    );

    // Serialize "decide script_id + insert" against concurrent create/move on the
    // same (workspace, path) via a transaction-scoped advisory lock, so two writers
    // can't end up with two script_ids at one path.
    let mut tx = db.begin().await?;
    lock_path(&mut tx, &workspace_id, &req.path).await?;

    // Find the latest version at this path: its hash seeds parent_hashes, and its
    // script_id is reused so all versions of a logical script share one stable id.
    // A brand-new path gets a fresh script_id.
    let prev = sqlx::query!(
        "SELECT hash, script_id FROM script WHERE workspace_id = $1 AND path = $2 ORDER BY created_at DESC LIMIT 1",
        workspace_id,
        req.path
    )
    .fetch_optional(&mut *tx)
    .await?;
    let parent_hashes: Option<Vec<String>> = prev.as_ref().map(|r| vec![r.hash.clone()]);
    let script_id = prev
        .as_ref()
        .map(|r| r.script_id)
        .unwrap_or_else(uuid::Uuid::new_v4);

    sqlx::query!(
        // Hash excludes name/summary, so re-saving with the same content but a
        // changed display name/summary collides on (workspace_id, hash). Update
        // the mutable, non-identity fields rather than silently dropping the edit.
        // (schema is derived from content, so it's unchanged on a hash match.)
        r#"INSERT INTO script (workspace_id, hash, path, name, content, language, runtime, parent_hashes, summary, requirements, created_by, schema, script_id)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
           ON CONFLICT (workspace_id, hash) DO UPDATE
             SET name = EXCLUDED.name,
                 summary = EXCLUDED.summary,
                 schema = EXCLUDED.schema"#,
        workspace_id,
        hash,
        req.path,
        name,
        req.content,
        lang_str,
        req.runtime.as_deref(),
        parent_hashes.as_deref(),
        summary,
        &requirements,
        user.email,
        schema,
        script_id
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok((StatusCode::CREATED, Json(ScriptCreated { hash })).into_response())
}

#[derive(serde::Deserialize)]
pub struct MoveScriptRequest {
    pub script_id: uuid::Uuid,
    pub new_path: String,
    /// Replace an existing script at the target path (requires write there).
    #[serde(default)]
    pub overwrite: bool,
}

/// Move/rename a script: change its path (and derived name). The stable
/// `script_id` is unchanged, so flow references survive. Requires write on both
/// the current location and the destination.
#[tracing::instrument(name = "api::move_script", skip(db, user, req), fields(%workspace_id))]
pub async fn move_script(
    State(db): State<PgPool>,
    Extension(user): Extension<AuthedUser>,
    Path(workspace_id): Path<String>,
    Json(req): Json<MoveScriptRequest>,
) -> Result<Response, ApiError> {
    let new_path = req.new_path.trim().to_string();
    if new_path.is_empty() {
        return Err(ApiError::BadRequest("new_path must not be empty".into()));
    }
    let new_name = new_path.rsplit('/').next().unwrap_or(&new_path).to_string();
    if new_name.is_empty() {
        return Err(ApiError::BadRequest(
            "new_path must not end with '/'".into(),
        ));
    }

    let current_path = sqlx::query_scalar!(
        "SELECT path FROM script WHERE workspace_id = $1 AND script_id = $2 ORDER BY created_at DESC LIMIT 1",
        workspace_id,
        req.script_id
    )
    .fetch_optional(&db)
    .await?
    .ok_or(ApiError::NotFound)?;

    if !user.is_valid_root_path(&new_path) {
        return Err(ApiError::BadRequest(
            "new_path must be under users/<you>/, teams/<your team>/, or workspace/".into(),
        ));
    }
    // Need write on the source (move out of it) and the destination (move into it).
    user.require_writer(&current_path)?;
    user.require_writer(&new_path)?;

    let mut tx = db.begin().await?;
    // Serialize the collision check + write against concurrent create/move.
    lock_path(&mut tx, &workspace_id, &new_path).await?;
    // Reject (or overwrite) a collision with a *different* logical script.
    let occupant: Option<uuid::Uuid> = sqlx::query_scalar!(
        "SELECT script_id FROM script WHERE workspace_id = $1 AND path = $2 LIMIT 1",
        workspace_id,
        new_path
    )
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(other) = occupant {
        if other != req.script_id {
            if !req.overwrite {
                // 409 with the occupant's references so the UI can warn before overwrite.
                let (referenced_by, active_runs) = script_refs(&db, &workspace_id, other).await?;
                return Ok((
                    StatusCode::CONFLICT,
                    Json(serde_json::json!({
                        "error": format!("target path '{new_path}' already exists"),
                        "referenced_by": referenced_by,
                        "active_runs": active_runs
                    })),
                )
                    .into_response());
            }
            sqlx::query!(
                "DELETE FROM script WHERE workspace_id = $1 AND script_id = $2",
                workspace_id,
                other
            )
            .execute(&mut *tx)
            .await?;
        }
    }

    sqlx::query!(
        "UPDATE script SET path = $1, name = $2 WHERE workspace_id = $3 AND script_id = $4",
        new_path,
        new_name,
        workspace_id,
        req.script_id
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

/// What references a script_id: the **current** flow definitions (latest revision
/// per path) that mention it, plus whether any **in-flight** run still references
/// it (a flow run whose `flow_value` snapshot uses the id, or a queued script
/// child run pinned to one of this script's hashes). script_id is a UUID, so a
/// JSON substring match won't false-positive.
async fn script_refs(
    db: &PgPool,
    workspace_id: &str,
    script_id: uuid::Uuid,
) -> Result<(Vec<String>, bool), ApiError> {
    let pattern = format!("%{script_id}%");
    let flows = sqlx::query_scalar!(
        r#"SELECT path FROM (
               SELECT DISTINCT ON (path) path, value
               FROM flow WHERE workspace_id = $1
               ORDER BY path, revision DESC
           ) latest
           WHERE value::text LIKE $2
           ORDER BY path"#,
        workspace_id,
        pattern
    )
    .fetch_all(db)
    .await?;

    let hashes: Vec<String> = sqlx::query_scalar!(
        "SELECT hash FROM script WHERE workspace_id = $1 AND script_id = $2",
        workspace_id,
        script_id
    )
    .fetch_all(db)
    .await?;

    let active = sqlx::query_scalar!(
        r#"SELECT EXISTS(
               SELECT 1 FROM run r JOIN run_queue q ON q.id = r.id
               WHERE r.workspace_id = $1 AND (
                   (r.kind IN ('flow', 'flow_preview') AND r.flow_value::text LIKE $2)
                   OR (r.script_hash = ANY($3))
               )
           ) AS "active!""#,
        workspace_id,
        pattern,
        &hashes
    )
    .fetch_one(db)
    .await?;

    Ok((flows, active))
}

#[tracing::instrument(name = "api::script_references", skip(db, user), fields(%workspace_id, %script_id))]
pub async fn script_references(
    State(db): State<PgPool>,
    Extension(user): Extension<AuthedUser>,
    Path((workspace_id, script_id)): Path<(String, uuid::Uuid)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Require read on the script's own path (don't let any member enumerate
    // references for arbitrary ids).
    let path = sqlx::query_scalar!(
        "SELECT path FROM script WHERE workspace_id = $1 AND script_id = $2 ORDER BY created_at DESC LIMIT 1",
        workspace_id,
        script_id
    )
    .fetch_optional(&db)
    .await?
    .ok_or(ApiError::NotFound)?;
    user.require_reader(&path)?;

    let (flows, active_runs) = script_refs(&db, &workspace_id, script_id).await?;
    Ok(Json(
        serde_json::json!({ "flows": flows, "active_runs": active_runs }),
    ))
}

#[derive(serde::Deserialize)]
pub struct DeleteScriptRequest {
    pub script_id: uuid::Uuid,
    /// Delete even if flows reference it (those nodes will fail to resolve at run).
    #[serde(default)]
    pub force: bool,
}

/// Delete all versions of a script (by stable id). Requires write on its folder.
/// Blocks with 409 + the referencing flow list unless `force`.
#[tracing::instrument(name = "api::delete_script", skip(db, user, req), fields(%workspace_id))]
pub async fn delete_script(
    State(db): State<PgPool>,
    Extension(user): Extension<AuthedUser>,
    Path(workspace_id): Path<String>,
    Json(req): Json<DeleteScriptRequest>,
) -> Result<Response, ApiError> {
    let path = sqlx::query_scalar!(
        "SELECT path FROM script WHERE workspace_id = $1 AND script_id = $2 ORDER BY created_at DESC LIMIT 1",
        workspace_id,
        req.script_id
    )
    .fetch_optional(&db)
    .await?
    .ok_or(ApiError::NotFound)?;
    user.require_writer(&path)?;

    if !req.force {
        let (referenced_by, active_runs) = script_refs(&db, &workspace_id, req.script_id).await?;
        if !referenced_by.is_empty() || active_runs {
            return Ok((
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error": "script is referenced by flows or in-flight runs",
                    "referenced_by": referenced_by,
                    "active_runs": active_runs
                })),
            )
                .into_response());
        }
    }

    sqlx::query!(
        "DELETE FROM script WHERE workspace_id = $1 AND script_id = $2",
        workspace_id,
        req.script_id
    )
    .execute(&db)
    .await?;
    Ok((StatusCode::OK, Json(serde_json::json!({ "deleted": true }))).into_response())
}

#[tracing::instrument(name = "api::list_scripts", skip(db, _user, query), fields(%workspace_id))]
pub async fn list_scripts(
    State(db): State<PgPool>,
    Extension(_user): Extension<AuthedUser>,
    Path(workspace_id): Path<String>,
    Query(query): Query<ListScriptsQuery>,
) -> Result<Json<Vec<ScriptListItem>>, ApiError> {
    let rows = match &query.path_prefix {
        Some(prefix) => {
            let pattern = format!("{prefix}%");
            sqlx::query_as!(
            ScriptListItem,
                r#"SELECT DISTINCT ON (path) hash, script_id, path, name as "name!", language, summary as "summary!", created_by, created_at as "created_at!"
                   FROM script
                   WHERE workspace_id = $1 AND path LIKE $2
                   ORDER BY path, created_at DESC"#,
                workspace_id,
                pattern
            )
            .fetch_all(&db)
            .await?
        }
        None => {
            sqlx::query_as!(
                ScriptListItem,
                r#"SELECT DISTINCT ON (path) hash, script_id, path, name as "name!", language, summary as "summary!", created_by, created_at as "created_at!"
                   FROM script
                   WHERE workspace_id = $1
                   ORDER BY path, created_at DESC"#,
                workspace_id
            )
            .fetch_all(&db)
            .await?
        }
    };

    Ok(Json(rows))
}

#[tracing::instrument(name = "api::list_script_versions", skip(db, user, query), fields(%workspace_id, %path))]
pub async fn list_script_versions(
    State(db): State<PgPool>,
    Extension(user): Extension<AuthedUser>,
    Path((workspace_id, path)): Path<(String, String)>,
    Query(query): Query<ScriptVersionsQuery>,
) -> Result<Json<ScriptVersionsResponse>, ApiError> {
    user.require_reader(&path)?;

    let (limit, offset) = script_versions_page(&query)?;

    let total = sqlx::query_scalar!(
        r#"SELECT COUNT(*) as "count!" FROM script WHERE workspace_id = $1 AND path = $2"#,
        workspace_id,
        path
    )
    .fetch_one(&db)
    .await?;

    let items = sqlx::query_as!(
        ScriptVersionItem,
        r#"SELECT hash, summary as "summary!", created_by, created_at as "created_at!", parent_hashes
           FROM script
           WHERE workspace_id = $1 AND path = $2
           ORDER BY created_at DESC
           LIMIT $3 OFFSET $4"#,
        workspace_id,
        path,
        limit,
        offset
    )
    .fetch_all(&db)
    .await?;

    let has_more = offset + (items.len() as i64) < total;

    Ok(Json(ScriptVersionsResponse {
        items,
        total,
        has_more,
    }))
}

#[tracing::instrument(name = "api::get_script_by_hash", skip(db, user), fields(%workspace_id, %hash))]
pub async fn get_script_by_hash(
    State(db): State<PgPool>,
    Extension(user): Extension<AuthedUser>,
    Path((workspace_id, hash)): Path<(String, String)>,
) -> Result<Json<ScriptResponse>, ApiError> {
    let row = sqlx::query!(
        r#"SELECT workspace_id, hash, script_id, path, name as "name!", content, language, runtime, schema,
                  parent_hashes, summary as "summary!", requirements, created_by,
                  created_at as "created_at!"
           FROM script
           WHERE workspace_id = $1 AND hash = $2"#,
        workspace_id,
        hash
    )
    .fetch_optional(&db)
    .await?
    .ok_or(ApiError::NotFound)?;

    user.require_reader(&row.path)?;

    Ok(Json(ScriptResponse {
        workspace_id: row.workspace_id,
        hash: row.hash,
        script_id: row.script_id,
        path: row.path,
        name: row.name,
        content: row.content,
        language: row.language,
        runtime: row.runtime,
        schema: row.schema,
        parent_hashes: row.parent_hashes,
        summary: row.summary,
        requirements: row.requirements,
        created_by: row.created_by,
        created_at: row.created_at,
    }))
}

#[tracing::instrument(name = "api::get_script_by_path", skip(db, user), fields(%workspace_id, %path))]
pub async fn get_script_by_path(
    State(db): State<PgPool>,
    Extension(user): Extension<AuthedUser>,
    Path((workspace_id, path)): Path<(String, String)>,
) -> Result<Json<ScriptResponse>, ApiError> {
    user.require_reader(&path)?;

    let row = sqlx::query!(
        r#"SELECT workspace_id, hash, script_id, path, name as "name!", content, language, runtime, schema,
                  parent_hashes, summary as "summary!", requirements, created_by,
                  created_at as "created_at!"
           FROM script
           WHERE workspace_id = $1 AND path = $2
           ORDER BY created_at DESC
           LIMIT 1"#,
        workspace_id,
        path
    )
    .fetch_optional(&db)
    .await?
    .ok_or(ApiError::NotFound)?;

    Ok(Json(ScriptResponse {
        workspace_id: row.workspace_id,
        hash: row.hash,
        script_id: row.script_id,
        path: row.path,
        name: row.name,
        content: row.content,
        language: row.language,
        runtime: row.runtime,
        schema: row.schema,
        parent_hashes: row.parent_hashes,
        summary: row.summary,
        requirements: row.requirements,
        created_by: row.created_by,
        created_at: row.created_at,
    }))
}

#[cfg(test)]
mod http_tests {
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use sqlx::PgPool;
    use tower::ServiceExt;

    use crate::test_helpers::*;

    fn create_body(path: &str, name: &str, content: &str) -> String {
        serde_json::json!({
            "path": path,
            "name": name,
            "content": content,
            "language": "python3"
        })
        .to_string()
    }

    async fn post_create(app: &axum::Router, token: &str, body: String) -> StatusCode {
        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/workspaces/ws-1/scripts/create")
                    .header("Authorization", format!("Bearer {token}"))
                    .header("Content-Type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn name_is_derived_from_path_leaf(pool: PgPool) {
        let user = "dev@test.local";
        seed_account(&pool, user).await;
        seed_workspace_member(&pool, "ws-1", user, "admin").await;
        let token = valid_jwt(user);
        let app = crate::create_router(
            pool.clone(),
            test_metrics(),
            crate::test_helpers::test_secret_key(),
        );

        let content = "def main():\n    return 1\n";
        // The request's `name` is ignored — name is always the path's leaf.
        assert_eq!(
            post_create(
                &app,
                &token,
                create_body("workspace/folder/close", "Ignored", content)
            )
            .await,
            StatusCode::CREATED
        );

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/workspaces/ws-1/scripts/get/path/workspace/folder/close")
                    .header("Authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["name"], "close");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn script_without_main_is_rejected(pool: PgPool) {
        let user = "dev@test.local";
        seed_account(&pool, user).await;
        seed_workspace_member(&pool, "ws-1", user, "admin").await;
        let token = valid_jwt(user);
        let app = crate::create_router(
            pool.clone(),
            test_metrics(),
            crate::test_helpers::test_secret_key(),
        );

        let status = post_create(
            &app,
            &token,
            create_body("workspace/lib", "Lib", "x = 1\nprint(x)\n"),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    async fn get_json(
        app: &axum::Router,
        token: &str,
        path: &str,
    ) -> (StatusCode, serde_json::Value) {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/workspaces/ws-1/scripts/get/path/{path}"))
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

    #[sqlx::test(migrations = "../../migrations")]
    async fn move_changes_path_and_name(pool: PgPool) {
        let user = "dev@test.local";
        seed_account(&pool, user).await;
        seed_workspace_member(&pool, "ws-1", user, "admin").await;
        let token = valid_jwt(user);
        let app = crate::create_router(
            pool.clone(),
            test_metrics(),
            crate::test_helpers::test_secret_key(),
        );

        let content = "def main():\n    return 1\n";
        assert_eq!(
            post_create(&app, &token, create_body("workspace/a/x", "x", content)).await,
            StatusCode::CREATED
        );
        let (_, before) = get_json(&app, &token, "workspace/a/x").await;
        let script_id = before["script_id"].as_str().unwrap().to_string();

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/workspaces/ws-1/scripts/move")
                    .header("Authorization", format!("Bearer {token}"))
                    .header("Content-Type", "application/json")
                    .body(Body::from(
                        serde_json::json!({ "script_id": script_id, "new_path": "workspace/b/y" })
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Old path gone, new path has the script with name = new leaf, same id.
        assert_eq!(
            get_json(&app, &token, "workspace/a/x").await.0,
            StatusCode::NOT_FOUND
        );
        let (status, after) = get_json(&app, &token, "workspace/b/y").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(after["name"], "y");
        assert_eq!(after["script_id"].as_str().unwrap(), script_id);
    }

    async fn post_json(
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

    async fn create_and_get_id(app: &axum::Router, token: &str, path: &str) -> String {
        assert_eq!(
            post_create(
                app,
                token,
                create_body(path, "x", "def main():\n    return 1\n")
            )
            .await,
            StatusCode::CREATED
        );
        let (_, json) = get_json(app, token, path).await;
        json["script_id"].as_str().unwrap().to_string()
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn delete_removes_script(pool: PgPool) {
        let user = "dev@test.local";
        seed_account(&pool, user).await;
        seed_workspace_member(&pool, "ws-1", user, "admin").await;
        let token = valid_jwt(user);
        let app = crate::create_router(
            pool.clone(),
            test_metrics(),
            crate::test_helpers::test_secret_key(),
        );

        let id = create_and_get_id(&app, &token, "workspace/a/x").await;
        let (status, _) = post_json(
            &app,
            &token,
            "/api/workspaces/ws-1/scripts/delete",
            serde_json::json!({ "script_id": id }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            get_json(&app, &token, "workspace/a/x").await.0,
            StatusCode::NOT_FOUND
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn delete_blocked_when_referenced_unless_force(pool: PgPool) {
        let user = "dev@test.local";
        seed_account(&pool, user).await;
        seed_workspace_member(&pool, "ws-1", user, "admin").await;
        let token = valid_jwt(user);
        let app = crate::create_router(
            pool.clone(),
            test_metrics(),
            crate::test_helpers::test_secret_key(),
        );

        let id = create_and_get_id(&app, &token, "workspace/a/x").await;
        // A flow referencing the script by id.
        let flow_value = serde_json::json!({
            "nodes": [{ "id": "n", "body": { "kind": "script", "script_id": id } }],
            "edges": []
        });
        assert_eq!(
            post_json(
                &app,
                &token,
                "/api/workspaces/ws-1/flows/create",
                serde_json::json!({ "path": "workspace/myflow", "value": flow_value })
            )
            .await
            .0,
            StatusCode::CREATED
        );

        // references endpoint lists the flow.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/workspaces/ws-1/scripts/references/{id}"))
                    .header("Authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let refs: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(refs["flows"], serde_json::json!(["workspace/myflow"]));

        // Delete without force → 409.
        let (status, body) = post_json(
            &app,
            &token,
            "/api/workspaces/ws-1/scripts/delete",
            serde_json::json!({ "script_id": id }),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(
            body["referenced_by"],
            serde_json::json!(["workspace/myflow"])
        );

        // Delete with force → 200, script gone.
        let (status, _) = post_json(
            &app,
            &token,
            "/api/workspaces/ws-1/scripts/delete",
            serde_json::json!({ "script_id": id, "force": true }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            get_json(&app, &token, "workspace/a/x").await.0,
            StatusCode::NOT_FOUND
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn move_collision_rejected_then_overwrites(pool: PgPool) {
        let user = "dev@test.local";
        seed_account(&pool, user).await;
        seed_workspace_member(&pool, "ws-1", user, "admin").await;
        let token = valid_jwt(user);
        let app = crate::create_router(
            pool.clone(),
            test_metrics(),
            crate::test_helpers::test_secret_key(),
        );

        let src = create_and_get_id(&app, &token, "workspace/a/x").await;
        let _dst = create_and_get_id(&app, &token, "workspace/b/y").await;

        // Move src onto the occupied path → 409 (no overwrite).
        let (status, _) = post_json(
            &app,
            &token,
            "/api/workspaces/ws-1/scripts/move",
            serde_json::json!({ "script_id": src, "new_path": "workspace/b/y" }),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);

        // With overwrite → 200; the path now resolves to the moved (src) script.
        let (status, _) = post_json(
            &app,
            &token,
            "/api/workspaces/ws-1/scripts/move",
            serde_json::json!({ "script_id": src, "new_path": "workspace/b/y", "overwrite": true }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (_, after) = get_json(&app, &token, "workspace/b/y").await;
        assert_eq!(after["script_id"].as_str().unwrap(), src);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn move_requires_write_on_source(pool: PgPool) {
        let admin = "admin@test.local";
        seed_account(&pool, admin).await;
        seed_workspace_member(&pool, "ws-1", admin, "admin").await;
        let bob = "bob@test.local";
        seed_account(&pool, bob).await;
        seed_workspace_member(&pool, "ws-1", bob, "editor").await;
        let app = crate::create_router(
            pool.clone(),
            test_metrics(),
            crate::test_helpers::test_secret_key(),
        );

        // Admin creates a script under alice's personal path; bob can't write there.
        let id = create_and_get_id(&app, &valid_jwt(admin), "users/alice@test.local/x").await;

        // Bob (non-admin, no write on the source) → 403.
        let (status, _) = post_json(
            &app,
            &valid_jwt(bob),
            "/api/workspaces/ws-1/scripts/move",
            serde_json::json!({ "script_id": id, "new_path": "users/bob@test.local/x" }),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn delete_blocked_by_in_flight_run_without_flow_def(pool: PgPool) {
        let user = "dev@test.local";
        seed_account(&pool, user).await;
        seed_workspace_member(&pool, "ws-1", user, "admin").await;
        let token = valid_jwt(user);
        let app = crate::create_router(
            pool.clone(),
            test_metrics(),
            crate::test_helpers::test_secret_key(),
        );

        let id = create_and_get_id(&app, &token, "workspace/a/x").await;

        // An in-flight flow run whose snapshot references the script — but there is
        // no flow definition referencing it, so only the run-snapshot check catches it.
        let run_id = uuid::Uuid::new_v4();
        let flow_value = serde_json::json!({
            "nodes": [{ "id": "n", "body": { "kind": "script", "script_id": id } }],
            "edges": []
        });
        sqlx::query!(
            "INSERT INTO run (id, workspace_id, kind, created_by, flow_value) VALUES ($1, 'ws-1', 'flow', $2, $3)",
            run_id,
            user,
            flow_value
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query!("INSERT INTO run_queue (id) VALUES ($1)", run_id)
            .execute(&pool)
            .await
            .unwrap();

        let (status, body) = post_json(
            &app,
            &token,
            "/api/workspaces/ws-1/scripts/delete",
            serde_json::json!({ "script_id": id }),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["active_runs"], serde_json::json!(true));
        assert_eq!(body["referenced_by"], serde_json::json!([]));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn concurrent_move_to_same_path_serializes(pool: PgPool) {
        let user = "dev@test.local";
        seed_account(&pool, user).await;
        seed_workspace_member(&pool, "ws-1", user, "admin").await;
        let token = valid_jwt(user);
        let app = crate::create_router(
            pool.clone(),
            test_metrics(),
            crate::test_helpers::test_secret_key(),
        );

        let a = create_and_get_id(&app, &token, "workspace/a/x").await;
        let b = create_and_get_id(&app, &token, "workspace/b/y").await;

        // Two scripts race to the same destination; the advisory lock must let only
        // one land there (the other gets a 409 collision), never both.
        let m1 = post_json(
            &app,
            &token,
            "/api/workspaces/ws-1/scripts/move",
            serde_json::json!({ "script_id": a, "new_path": "workspace/c/z" }),
        );
        let m2 = post_json(
            &app,
            &token,
            "/api/workspaces/ws-1/scripts/move",
            serde_json::json!({ "script_id": b, "new_path": "workspace/c/z" }),
        );
        let ((s1, _), (s2, _)) = tokio::time::timeout(std::time::Duration::from_secs(20), async {
            tokio::join!(m1, m2)
        })
        .await
        .expect("no deadlock");

        let mut statuses = [s1, s2];
        statuses.sort_by_key(|s| s.as_u16());
        assert_eq!(statuses, [StatusCode::OK, StatusCode::CONFLICT]);

        let n: i64 = sqlx::query_scalar!(
            "SELECT count(DISTINCT script_id) FROM script WHERE workspace_id = 'ws-1' AND path = 'workspace/c/z'"
        )
        .fetch_one(&pool)
        .await
        .unwrap()
        .unwrap_or(0);
        assert_eq!(n, 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_script_hash_deterministic() {
        let h1 = compute_script_hash(
            "print('hello')",
            "users/alice/hello.py",
            "python3",
            None,
            &[],
        );
        let h2 = compute_script_hash(
            "print('hello')",
            "users/alice/hello.py",
            "python3",
            None,
            &[],
        );
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
    }

    #[test]
    fn test_compute_script_hash_different_content() {
        let h1 = compute_script_hash(
            "print('hello')",
            "users/alice/hello.py",
            "python3",
            None,
            &[],
        );
        let h2 = compute_script_hash(
            "print('world')",
            "users/alice/hello.py",
            "python3",
            None,
            &[],
        );
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_compute_script_hash_different_path() {
        let h1 = compute_script_hash("print('hello')", "users/alice/a.py", "python3", None, &[]);
        let h2 = compute_script_hash("print('hello')", "users/alice/b.py", "python3", None, &[]);
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_compute_script_hash_different_runtime() {
        let h1 = compute_script_hash("print(1)", "a.py", "python3", Some("python:3.12"), &[]);
        let h2 = compute_script_hash("print(1)", "a.py", "python3", Some("python:3.11"), &[]);
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_compute_script_hash_different_requirements() {
        let r1 = vec!["requests".to_string()];
        let r2 = vec!["pandas".to_string()];
        let h1 = compute_script_hash("print(1)", "a.py", "python3", None, &r1);
        let h2 = compute_script_hash("print(1)", "a.py", "python3", None, &r2);
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_compute_script_hash_none_vs_some_runtime() {
        let h1 = compute_script_hash("print(1)", "a.py", "python3", None, &[]);
        let h2 = compute_script_hash("print(1)", "a.py", "python3", Some("python:3.12"), &[]);
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_validate_script_runtime_and_requirements_accepts_python_options() {
        let requirements = vec!["requests".to_string()];

        assert!(
            validate_script_runtime_and_requirements(
                &ScriptLang::Python3,
                Some("python:3.12"),
                &requirements,
            )
            .is_ok()
        );
    }

    #[test]
    fn test_script_versions_page_defaults() {
        let (limit, offset) = script_versions_page(&ScriptVersionsQuery {
            limit: None,
            offset: None,
        })
        .unwrap();

        assert_eq!(limit, 20);
        assert_eq!(offset, 0);
    }

    #[test]
    fn test_script_versions_page_caps_limit() {
        let (limit, offset) = script_versions_page(&ScriptVersionsQuery {
            limit: Some(500),
            offset: Some(10),
        })
        .unwrap();

        assert_eq!(limit, 100);
        assert_eq!(offset, 10);
    }

    #[test]
    fn test_script_versions_page_rejects_invalid_values() {
        assert!(
            script_versions_page(&ScriptVersionsQuery {
                limit: Some(0),
                offset: None,
            })
            .is_err()
        );
        assert!(
            script_versions_page(&ScriptVersionsQuery {
                limit: None,
                offset: Some(-1),
            })
            .is_err()
        );
    }
}
