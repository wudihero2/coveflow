use axum::Extension;
use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use sqlx::PgPool;

use crate::auth::{self, AuthedUser, WorkspaceRole};
use crate::error::ApiError;

#[derive(serde::Serialize)]
pub struct WorkspaceInfo {
    id: String,
    name: String,
    owner: String,
}

#[derive(serde::Serialize)]
pub(crate) struct MeResponse {
    email: String,
    role: WorkspaceRole,
    is_instance_admin: bool,
    /// Teams the user belongs to (their `teams/<name>/` roots).
    teams: Vec<String>,
    /// Subset of `teams` the user can write to (writer/owner role) — lets the
    /// frontend mirror `can_write` for `teams/<name>/` paths.
    writable_teams: Vec<String>,
}

#[tracing::instrument(name = "api::list_workspaces", skip(db, headers))]
pub async fn list_workspaces(
    State(db): State<PgPool>,
    headers: HeaderMap,
) -> Result<Json<Vec<WorkspaceInfo>>, ApiError> {
    let email = auth::email_from_bearer_headers(&headers)?;
    let rows = sqlx::query_as!(
        WorkspaceInfo,
        r#"SELECT w.id, w.name, w.owner
           FROM workspace w
           JOIN workspace_member wm ON wm.workspace_id = w.id
           WHERE wm.email = $1
           ORDER BY w.created_at ASC, w.id ASC"#,
        email
    )
    .fetch_all(&db)
    .await?;

    Ok(Json(rows))
}

#[tracing::instrument(name = "api::get_me", skip(db, user))]
pub(crate) async fn get_me(
    State(db): State<PgPool>,
    Extension(user): Extension<AuthedUser>,
) -> Result<Json<MeResponse>, ApiError> {
    // The instance-admin flag lives on `account`, not on workspace membership, and
    // only `/me` needs it — so look it up here rather than JOINing `account` into
    // every authenticated workspace request in require_auth. A missing account row
    // (shouldn't happen for a member) defaults to non-admin rather than 500ing.
    let is_instance_admin = sqlx::query_scalar!(
        r#"SELECT is_admin AS "is_admin!" FROM account WHERE email = $1"#,
        user.email
    )
    .fetch_optional(&db)
    .await?
    .unwrap_or(false);

    let writable_teams: Vec<String> = user
        .team_roles
        .iter()
        .filter(|(_, r)| {
            matches!(
                r,
                crate::auth::FolderRole::Writer | crate::auth::FolderRole::Owner
            )
        })
        .map(|(t, _)| t.clone())
        .collect();

    Ok(Json(MeResponse {
        email: user.email,
        role: user.role,
        is_instance_admin,
        teams: user.teams,
        writable_teams,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_user(email: &str, role: WorkspaceRole) -> AuthedUser {
        AuthedUser {
            email: email.to_string(),
            workspace_id: "ws-1".to_string(),
            role,
            teams: Vec::new(),
            team_roles: HashMap::new(),
        }
    }

    async fn seed_account(pool: &PgPool, email: &str, is_admin: bool) {
        sqlx::query!(
            "INSERT INTO account (email, password_hash, is_admin) VALUES ($1, '', $2)",
            email,
            is_admin
        )
        .execute(pool)
        .await
        .unwrap();
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn get_me_reports_role_and_instance_admin(pool: PgPool) {
        seed_account(&pool, "admin@example.com", true).await;
        let user = make_user("admin@example.com", WorkspaceRole::Admin);
        let Json(resp) = get_me(State(pool), Extension(user)).await.unwrap();

        assert_eq!(resp.email, "admin@example.com");
        assert!(resp.is_instance_admin);
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["role"], "admin");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn get_me_non_admin_account_is_not_instance_admin(pool: PgPool) {
        seed_account(&pool, "editor@example.com", false).await;
        let user = make_user("editor@example.com", WorkspaceRole::Editor);
        let Json(resp) = get_me(State(pool), Extension(user)).await.unwrap();

        assert!(!resp.is_instance_admin);
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["role"], "editor");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn get_me_missing_account_defaults_to_non_admin(pool: PgPool) {
        // No account row for this email: must default to non-admin, not error.
        let user = make_user("ghost@example.com", WorkspaceRole::Viewer);
        let Json(resp) = get_me(State(pool), Extension(user)).await.unwrap();

        assert!(!resp.is_instance_admin);
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["role"], "viewer");
    }
}
