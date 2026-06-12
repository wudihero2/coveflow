use axum::Extension;
use axum::Json;
use axum::extract::{Path, State};
use sqlx::PgPool;

use crate::auth::AuthedUser;
use crate::error::ApiError;

const VALID_ROLES: &[&str] = &["admin", "editor", "viewer", "operator"];

fn validate_role(role: &str) -> Result<(), ApiError> {
    if VALID_ROLES.contains(&role) {
        Ok(())
    } else {
        Err(ApiError::BadRequest("invalid role".into()))
    }
}

#[derive(serde::Serialize)]
pub struct MemberItem {
    pub email: String,
    pub role: String,
}

#[derive(serde::Deserialize)]
pub struct AddMemberRequest {
    pub email: String,
    pub role: String,
}

#[derive(serde::Deserialize)]
pub struct UpdateMemberRoleRequest {
    pub role: String,
}

#[tracing::instrument(
    name = "api::list_members",
    skip(db, user),
    fields(%workspace_id, email = %user.email)
)]
pub async fn list_members(
    State(db): State<PgPool>,
    Extension(user): Extension<AuthedUser>,
    Path(workspace_id): Path<String>,
) -> Result<Json<Vec<MemberItem>>, ApiError> {
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin role required".into()));
    }

    let rows = sqlx::query_as!(
        MemberItem,
        r#"SELECT email, role
           FROM workspace_member
           WHERE workspace_id = $1
           ORDER BY email ASC"#,
        workspace_id,
    )
    .fetch_all(&db)
    .await?;

    Ok(Json(rows))
}

#[tracing::instrument(
    name = "api::add_member",
    skip(db, user, body),
    fields(%workspace_id, email = %user.email)
)]
pub async fn add_member(
    State(db): State<PgPool>,
    Extension(user): Extension<AuthedUser>,
    Path(workspace_id): Path<String>,
    Json(body): Json<AddMemberRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin role required".into()));
    }

    validate_role(&body.role)?;

    // Verify email exists in account table
    let account_exists = sqlx::query_scalar!(
        r#"SELECT EXISTS(
            SELECT 1 FROM account WHERE email = $1
        ) AS "exists!""#,
        body.email,
    )
    .fetch_one(&db)
    .await?;

    if !account_exists {
        return Err(ApiError::NotFound);
    }

    sqlx::query!(
        "INSERT INTO workspace_member (workspace_id, email, role) VALUES ($1, $2, $3)",
        workspace_id,
        body.email,
        body.role,
    )
    .execute(&db)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(ref db_err) if db_err.is_unique_violation() => {
            ApiError::Conflict("already a member".into())
        }
        other => ApiError::Db(other),
    })?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

#[tracing::instrument(
    name = "api::update_member_role",
    skip(db, user, body),
    fields(%workspace_id, %target_email, email = %user.email)
)]
pub async fn update_member_role(
    State(db): State<PgPool>,
    Extension(user): Extension<AuthedUser>,
    Path((workspace_id, target_email)): Path<(String, String)>,
    Json(body): Json<UpdateMemberRoleRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin role required".into()));
    }

    validate_role(&body.role)?;

    if user.email == target_email {
        return Err(ApiError::Forbidden("cannot change your own role".into()));
    }

    let result = sqlx::query!(
        "UPDATE workspace_member SET role = $3 WHERE workspace_id = $1 AND email = $2",
        workspace_id,
        target_email,
        body.role,
    )
    .execute(&db)
    .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

#[tracing::instrument(
    name = "api::remove_member",
    skip(db, user),
    fields(%workspace_id, %target_email, email = %user.email)
)]
pub async fn remove_member(
    State(db): State<PgPool>,
    Extension(user): Extension<AuthedUser>,
    Path((workspace_id, target_email)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin role required".into()));
    }

    if user.email == target_email {
        return Err(ApiError::Forbidden("cannot remove yourself".into()));
    }

    let mut tx = db.begin().await?;

    // Remove from workspace_member
    let result = sqlx::query!(
        "DELETE FROM workspace_member WHERE workspace_id = $1 AND email = $2",
        workspace_id,
        target_email,
    )
    .execute(&mut *tx)
    .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    // Clean up team_member (no FK cascade on email)
    sqlx::query!(
        "DELETE FROM team_member WHERE workspace_id = $1 AND email = $2",
        workspace_id,
        target_email,
    )
    .execute(&mut *tx)
    .await?;

    // Clean up folder_acl (subject is free text, no FK cascade)
    let user_subject = format!("users/{target_email}");
    sqlx::query!(
        "DELETE FROM folder_acl WHERE workspace_id = $1 AND subject = $2",
        workspace_id,
        user_subject,
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use sqlx::PgPool;

    use crate::test_helpers::*;

    #[sqlx::test(migrations = "../../migrations")]
    async fn test_list_members(pool: PgPool) {
        let admin = "admin@test.local";
        let editor = "editor@test.local";
        seed_account(&pool, admin).await;
        seed_account(&pool, editor).await;
        seed_workspace_member(&pool, "ws-1", admin, "admin").await;
        seed_workspace_member(&pool, "ws-1", editor, "editor").await;

        let (status, json) =
            call_json(pool, "GET", "/api/workspaces/ws-1/members", admin, None).await;
        assert_eq!(status, StatusCode::OK);
        let items = json.as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["email"], "admin@test.local");
        assert_eq!(items[0]["role"], "admin");
        assert_eq!(items[1]["email"], "editor@test.local");
        assert_eq!(items[1]["role"], "editor");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn test_list_members_non_admin_forbidden(pool: PgPool) {
        let editor = "editor@test.local";
        seed_account(&pool, editor).await;
        seed_workspace_member(&pool, "ws-1", editor, "editor").await;

        let (status, _) =
            call_json(pool, "GET", "/api/workspaces/ws-1/members", editor, None).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn test_add_member_success(pool: PgPool) {
        let admin = "admin@test.local";
        let new_user = "newuser@test.local";
        seed_account(&pool, admin).await;
        seed_account(&pool, new_user).await;
        seed_workspace_member(&pool, "ws-1", admin, "admin").await;

        let body = serde_json::json!({ "email": new_user, "role": "editor" });
        let (status, _) = call_json(
            pool.clone(),
            "POST",
            "/api/workspaces/ws-1/members",
            admin,
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // Verify member was added
        let (status, json) =
            call_json(pool, "GET", "/api/workspaces/ws-1/members", admin, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json.as_array().unwrap().len(), 2);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn test_add_member_email_not_found(pool: PgPool) {
        let admin = "admin@test.local";
        seed_account(&pool, admin).await;
        seed_workspace_member(&pool, "ws-1", admin, "admin").await;

        let body = serde_json::json!({ "email": "unknown@test.local", "role": "editor" });
        let (status, _) = call_json(
            pool,
            "POST",
            "/api/workspaces/ws-1/members",
            admin,
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn test_add_member_already_exists(pool: PgPool) {
        let admin = "admin@test.local";
        let existing = "existing@test.local";
        seed_account(&pool, admin).await;
        seed_account(&pool, existing).await;
        seed_workspace_member(&pool, "ws-1", admin, "admin").await;
        seed_workspace_member(&pool, "ws-1", existing, "editor").await;

        let body = serde_json::json!({ "email": existing, "role": "viewer" });
        let (status, _) = call_json(
            pool,
            "POST",
            "/api/workspaces/ws-1/members",
            admin,
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn test_add_member_invalid_role(pool: PgPool) {
        let admin = "admin@test.local";
        let new_user = "newuser@test.local";
        seed_account(&pool, admin).await;
        seed_account(&pool, new_user).await;
        seed_workspace_member(&pool, "ws-1", admin, "admin").await;

        let body = serde_json::json!({ "email": new_user, "role": "superadmin" });
        let (status, _) = call_json(
            pool,
            "POST",
            "/api/workspaces/ws-1/members",
            admin,
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn test_update_member_role(pool: PgPool) {
        let admin = "admin@test.local";
        let editor = "editor@test.local";
        seed_account(&pool, admin).await;
        seed_account(&pool, editor).await;
        seed_workspace_member(&pool, "ws-1", admin, "admin").await;
        seed_workspace_member(&pool, "ws-1", editor, "editor").await;

        let body = serde_json::json!({ "role": "viewer" });
        let (status, _) = call_json(
            pool,
            "PUT",
            "/api/workspaces/ws-1/members/editor@test.local",
            admin,
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn test_update_own_role_forbidden(pool: PgPool) {
        let admin = "admin@test.local";
        seed_account(&pool, admin).await;
        seed_workspace_member(&pool, "ws-1", admin, "admin").await;

        let body = serde_json::json!({ "role": "editor" });
        let (status, _) = call_json(
            pool,
            "PUT",
            "/api/workspaces/ws-1/members/admin@test.local",
            admin,
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn test_remove_member(pool: PgPool) {
        let admin = "admin@test.local";
        let editor = "editor@test.local";
        seed_account(&pool, admin).await;
        seed_account(&pool, editor).await;
        seed_workspace_member(&pool, "ws-1", admin, "admin").await;
        seed_workspace_member(&pool, "ws-1", editor, "editor").await;

        let (status, _) = call_json(
            pool.clone(),
            "DELETE",
            "/api/workspaces/ws-1/members/editor@test.local",
            admin,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // Verify member was removed
        let (status, json) =
            call_json(pool, "GET", "/api/workspaces/ws-1/members", admin, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json.as_array().unwrap().len(), 1);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn test_remove_self_forbidden(pool: PgPool) {
        let admin = "admin@test.local";
        seed_account(&pool, admin).await;
        seed_workspace_member(&pool, "ws-1", admin, "admin").await;

        let (status, _) = call_json(
            pool,
            "DELETE",
            "/api/workspaces/ws-1/members/admin@test.local",
            admin,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn test_remove_member_cleans_up_team_member(pool: PgPool) {
        let admin = "admin@test.local";
        let member = "member@test.local";
        seed_account(&pool, admin).await;
        seed_account(&pool, member).await;
        seed_workspace_member(&pool, "ws-1", admin, "admin").await;
        seed_workspace_member(&pool, "ws-1", member, "editor").await;
        seed_team(&pool, "ws-1", "ml-team", "").await;
        seed_team_member(&pool, "ws-1", member, "ml-team").await;

        // Remove member from workspace
        let (status, _) = call_json(
            pool.clone(),
            "DELETE",
            "/api/workspaces/ws-1/members/member@test.local",
            admin,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // Verify team_member was also cleaned up
        let count = sqlx::query_scalar!(
            r#"SELECT COUNT(*) AS "count!" FROM team_member
               WHERE workspace_id = 'ws-1' AND email = 'member@test.local'"#,
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 0);
    }
}
