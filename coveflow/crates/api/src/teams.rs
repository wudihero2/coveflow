use axum::Extension;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use sqlx::PgPool;

use crate::auth::AuthedUser;
use crate::common::validate_name;
use crate::error::ApiError;

#[derive(serde::Deserialize, Debug)]
pub struct DeleteTeamQuery {
    #[serde(default)]
    pub force: bool,
}

#[derive(serde::Serialize)]
pub struct TeamListItemWithCount {
    pub name: String,
    pub summary: String,
    pub member_count: i64,
}

#[derive(serde::Serialize)]
pub struct TeamListResponse {
    pub items: Vec<TeamListItemWithCount>,
}

#[derive(serde::Deserialize)]
pub struct CreateTeamRequest {
    pub name: String,
    pub summary: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct AddTeamMemberRequest {
    pub email: String,
    /// `reader` or `writer` (default `writer`). Controls write access to the
    /// team's `teams/<name>/` space.
    #[serde(default)]
    pub role: Option<String>,
}

#[derive(serde::Serialize)]
pub struct TeamMemberItem {
    pub email: String,
    pub role: String,
}

#[derive(serde::Deserialize)]
pub struct UpdateTeamMemberRoleRequest {
    pub role: String,
}

fn validate_team_role(role: &str) -> Result<(), ApiError> {
    if role == "reader" || role == "writer" {
        Ok(())
    } else {
        Err(ApiError::BadRequest(
            "role must be 'reader' or 'writer'".into(),
        ))
    }
}

#[derive(serde::Serialize)]
pub struct TeamQuotaResponse {
    pub max_concurrent_runs: Option<i32>,
    pub max_cpus: Option<f32>,
    pub max_memory_mb: Option<i64>,
    pub max_daily_runs: Option<i32>,
    pub max_storage_bytes: Option<i64>,
    pub max_run_timeout_secs: Option<i32>,
}

#[derive(serde::Deserialize)]
pub struct UpdateTeamQuotaRequest {
    pub max_concurrent_runs: Option<i32>,
    pub max_cpus: Option<f32>,
    pub max_memory_mb: Option<i64>,
    pub max_daily_runs: Option<i32>,
    pub max_storage_bytes: Option<i64>,
    pub max_run_timeout_secs: Option<i32>,
}

#[tracing::instrument(
    name = "api::list_teams",
    skip(db, user),
    fields(%workspace_id, email = %user.email)
)]
pub async fn list_teams(
    State(db): State<PgPool>,
    Extension(user): Extension<AuthedUser>,
    Path(workspace_id): Path<String>,
) -> Result<Json<TeamListResponse>, ApiError> {
    let items = if user.is_admin() {
        sqlx::query_as!(
            TeamListItemWithCount,
            r#"SELECT t.name,
                      COALESCE(t.summary, '') AS "summary!",
                      COUNT(tm.email)::bigint AS "member_count!"
               FROM team t
               LEFT JOIN team_member tm
                 ON tm.workspace_id = t.workspace_id
                AND tm.team_name = t.name
               WHERE t.workspace_id = $1
               GROUP BY t.workspace_id, t.name, t.summary
               ORDER BY t.name ASC"#,
            workspace_id,
        )
        .fetch_all(&db)
        .await?
    } else {
        sqlx::query_as!(
            TeamListItemWithCount,
            r#"SELECT t.name,
                      COALESCE(t.summary, '') AS "summary!",
                      COUNT(tm_all.email)::bigint AS "member_count!"
               FROM team t
               JOIN team_member tm_me
                 ON tm_me.workspace_id = t.workspace_id
                AND tm_me.team_name = t.name
                AND tm_me.email = $2
               LEFT JOIN team_member tm_all
                 ON tm_all.workspace_id = t.workspace_id
                AND tm_all.team_name = t.name
               WHERE t.workspace_id = $1
               GROUP BY t.workspace_id, t.name, t.summary
               ORDER BY t.name ASC"#,
            workspace_id,
            user.email,
        )
        .fetch_all(&db)
        .await?
    };

    Ok(Json(TeamListResponse { items }))
}

#[tracing::instrument(
    name = "api::get_team",
    skip(db, user),
    fields(%workspace_id, %name, email = %user.email)
)]
pub async fn get_team(
    State(db): State<PgPool>,
    Extension(user): Extension<AuthedUser>,
    Path((workspace_id, name)): Path<(String, String)>,
) -> Result<Json<TeamListItemWithCount>, ApiError> {
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin role required".into()));
    }

    let team = sqlx::query_as!(
        TeamListItemWithCount,
        r#"SELECT t.name,
                  COALESCE(t.summary, '') AS "summary!",
                  COUNT(tm.email)::bigint AS "member_count!"
           FROM team t
           LEFT JOIN team_member tm
             ON tm.workspace_id = t.workspace_id
            AND tm.team_name = t.name
           WHERE t.workspace_id = $1 AND t.name = $2
           GROUP BY t.workspace_id, t.name, t.summary"#,
        workspace_id,
        name,
    )
    .fetch_optional(&db)
    .await?
    .ok_or(ApiError::NotFound)?;

    Ok(Json(team))
}

#[tracing::instrument(
    name = "api::create_team",
    skip(db, user, body),
    fields(%workspace_id, email = %user.email)
)]
pub async fn create_team(
    State(db): State<PgPool>,
    Extension(user): Extension<AuthedUser>,
    Path(workspace_id): Path<String>,
    Json(body): Json<CreateTeamRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin role required".into()));
    }

    validate_name(&body.name)?;

    let mut tx = db.begin().await?;
    sqlx::query!(
        "INSERT INTO team (workspace_id, name, summary) VALUES ($1, $2, $3)",
        workspace_id,
        body.name,
        body.summary as Option<String>,
    )
    .execute(&mut *tx)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(ref db_err) if db_err.is_unique_violation() => {
            ApiError::Conflict("team already exists".into())
        }
        other => ApiError::Db(other),
    })?;

    // The creator joins as a writer, so the team's `teams/<name>/` space is
    // immediately usable + visible to them (no separate folder entity exists).
    sqlx::query!(
        "INSERT INTO team_member (workspace_id, email, team_name, role)
         VALUES ($1, $2, $3, 'writer')
         ON CONFLICT DO NOTHING",
        workspace_id,
        user.email,
        body.name,
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

#[tracing::instrument(
    name = "api::delete_team",
    skip(db, user),
    fields(%workspace_id, %name, email = %user.email)
)]
pub async fn delete_team(
    State(db): State<PgPool>,
    Extension(user): Extension<AuthedUser>,
    Path((workspace_id, name)): Path<(String, String)>,
    Query(q): Query<DeleteTeamQuery>,
) -> Result<Response, ApiError> {
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin role required".into()));
    }

    // The team owns the `teams/<name>/` space. Deleting it removes every member's
    // access (and hides the root), orphaning any scripts/flows there. Block unless
    // forced, so an admin can move that content out (to workspace/) first.
    let prefix = format!("teams/{name}/%");
    if !q.force {
        let scripts: i64 = sqlx::query_scalar!(
            r#"SELECT COUNT(DISTINCT path) AS "n!" FROM script WHERE workspace_id = $1 AND path LIKE $2"#,
            workspace_id,
            prefix,
        )
        .fetch_one(&db)
        .await?;
        let flows: i64 = sqlx::query_scalar!(
            r#"SELECT COUNT(DISTINCT path) AS "n!" FROM flow WHERE workspace_id = $1 AND path LIKE $2"#,
            workspace_id,
            prefix,
        )
        .fetch_one(&db)
        .await?;
        if scripts + flows > 0 {
            return Ok((
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error": "team space is not empty; move its files out (to workspace/) or force-delete",
                    "scripts": scripts,
                    "flows": flows,
                })),
            )
                .into_response());
        }
    }

    // team_member rows cascade via the FK; folder ACL is no longer used.
    let result = sqlx::query!(
        "DELETE FROM team WHERE workspace_id = $1 AND name = $2",
        workspace_id,
        name,
    )
    .execute(&db)
    .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    Ok((StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response())
}

#[tracing::instrument(
    name = "api::list_team_members",
    skip(db, user),
    fields(%workspace_id, %name, email = %user.email)
)]
pub async fn list_team_members(
    State(db): State<PgPool>,
    Extension(user): Extension<AuthedUser>,
    Path((workspace_id, name)): Path<(String, String)>,
) -> Result<Json<Vec<TeamMemberItem>>, ApiError> {
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin role required".into()));
    }

    let rows = sqlx::query!(
        r#"SELECT email, role FROM team_member
           WHERE workspace_id = $1 AND team_name = $2
           ORDER BY email ASC"#,
        workspace_id,
        name,
    )
    .fetch_all(&db)
    .await?;

    Ok(Json(
        rows.into_iter()
            .map(|r| TeamMemberItem {
                email: r.email,
                role: r.role,
            })
            .collect(),
    ))
}

#[tracing::instrument(
    name = "api::add_team_member",
    skip(db, user, body),
    fields(%workspace_id, %name, email = %user.email)
)]
pub async fn add_team_member(
    State(db): State<PgPool>,
    Extension(user): Extension<AuthedUser>,
    Path((workspace_id, name)): Path<(String, String)>,
    Json(body): Json<AddTeamMemberRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin role required".into()));
    }

    // Verify email is a workspace member
    let is_member = sqlx::query_scalar!(
        r#"SELECT EXISTS(
            SELECT 1 FROM workspace_member
            WHERE workspace_id = $1 AND email = $2
        ) AS "exists!""#,
        workspace_id,
        body.email,
    )
    .fetch_one(&db)
    .await?;

    if !is_member {
        return Err(ApiError::BadRequest(
            "user is not a workspace member".into(),
        ));
    }

    // Verify team exists
    let team_exists = sqlx::query_scalar!(
        r#"SELECT EXISTS(
            SELECT 1 FROM team
            WHERE workspace_id = $1 AND name = $2
        ) AS "exists!""#,
        workspace_id,
        name,
    )
    .fetch_one(&db)
    .await?;

    if !team_exists {
        return Err(ApiError::NotFound);
    }

    let role = body.role.as_deref().unwrap_or("writer");
    validate_team_role(role)?;

    sqlx::query!(
        "INSERT INTO team_member (workspace_id, email, team_name, role) VALUES ($1, $2, $3, $4)",
        workspace_id,
        body.email,
        name,
        role,
    )
    .execute(&db)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(ref db_err) if db_err.is_unique_violation() => {
            ApiError::Conflict("already a team member".into())
        }
        other => ApiError::Db(other),
    })?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// Change a team member's role (reader/writer). Admin-only.
#[tracing::instrument(
    name = "api::update_team_member_role",
    skip(db, user, body),
    fields(%workspace_id, %name, %target_email, email = %user.email)
)]
pub async fn update_team_member_role(
    State(db): State<PgPool>,
    Extension(user): Extension<AuthedUser>,
    Path((workspace_id, name, target_email)): Path<(String, String, String)>,
    Json(body): Json<UpdateTeamMemberRoleRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin role required".into()));
    }
    validate_team_role(&body.role)?;
    let res = sqlx::query!(
        "UPDATE team_member SET role = $1 WHERE workspace_id = $2 AND team_name = $3 AND email = $4",
        body.role,
        workspace_id,
        name,
        target_email,
    )
    .execute(&db)
    .await?;
    if res.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[tracing::instrument(
    name = "api::remove_team_member",
    skip(db, user),
    fields(%workspace_id, %name, %target_email, email = %user.email)
)]
pub async fn remove_team_member(
    State(db): State<PgPool>,
    Extension(user): Extension<AuthedUser>,
    Path((workspace_id, name, target_email)): Path<(String, String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin role required".into()));
    }

    let result = sqlx::query!(
        "DELETE FROM team_member WHERE workspace_id = $1 AND team_name = $2 AND email = $3",
        workspace_id,
        name,
        target_email,
    )
    .execute(&db)
    .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

#[tracing::instrument(
    name = "api::get_team_quota",
    skip(db, user),
    fields(%workspace_id, %name, email = %user.email)
)]
pub async fn get_team_quota(
    State(db): State<PgPool>,
    Extension(user): Extension<AuthedUser>,
    Path((workspace_id, name)): Path<(String, String)>,
) -> Result<Json<TeamQuotaResponse>, ApiError> {
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin role required".into()));
    }

    let row = sqlx::query!(
        r#"SELECT max_concurrent_runs, max_cpus, max_memory_mb,
                  max_daily_runs, max_storage_bytes, max_run_timeout_secs
           FROM team_quota
           WHERE workspace_id = $1 AND team_name = $2"#,
        workspace_id,
        name,
    )
    .fetch_optional(&db)
    .await?;

    let resp = match row {
        Some(r) => TeamQuotaResponse {
            max_concurrent_runs: r.max_concurrent_runs,
            max_cpus: r.max_cpus,
            max_memory_mb: r.max_memory_mb,
            max_daily_runs: r.max_daily_runs,
            max_storage_bytes: r.max_storage_bytes,
            max_run_timeout_secs: r.max_run_timeout_secs,
        },
        None => TeamQuotaResponse {
            max_concurrent_runs: None,
            max_cpus: None,
            max_memory_mb: None,
            max_daily_runs: None,
            max_storage_bytes: None,
            max_run_timeout_secs: None,
        },
    };

    Ok(Json(resp))
}

#[tracing::instrument(
    name = "api::update_team_quota",
    skip(db, user, body),
    fields(%workspace_id, %name, email = %user.email)
)]
pub async fn update_team_quota(
    State(db): State<PgPool>,
    Extension(user): Extension<AuthedUser>,
    Path((workspace_id, name)): Path<(String, String)>,
    Json(body): Json<UpdateTeamQuotaRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin role required".into()));
    }

    // Verify team exists
    let team_exists = sqlx::query_scalar!(
        r#"SELECT EXISTS(
            SELECT 1 FROM team
            WHERE workspace_id = $1 AND name = $2
        ) AS "exists!""#,
        workspace_id,
        name,
    )
    .fetch_one(&db)
    .await?;

    if !team_exists {
        return Err(ApiError::NotFound);
    }

    sqlx::query!(
        r#"INSERT INTO team_quota (
               workspace_id, team_name,
               max_concurrent_runs, max_cpus, max_memory_mb,
               max_daily_runs, max_storage_bytes, max_run_timeout_secs
           ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
           ON CONFLICT (workspace_id, team_name)
           DO UPDATE SET
               max_concurrent_runs = $3,
               max_cpus = $4,
               max_memory_mb = $5,
               max_daily_runs = $6,
               max_storage_bytes = $7,
               max_run_timeout_secs = $8"#,
        workspace_id,
        name,
        body.max_concurrent_runs,
        body.max_cpus,
        body.max_memory_mb,
        body.max_daily_runs,
        body.max_storage_bytes,
        body.max_run_timeout_secs,
    )
    .execute(&db)
    .await?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use sqlx::PgPool;
    use tower::ServiceExt;

    use crate::test_helpers::*;

    async fn call_list_teams(
        pool: PgPool,
        ws_id: &str,
        email: &str,
    ) -> (StatusCode, serde_json::Value) {
        let token = valid_jwt(email);
        let uri = format!("/api/workspaces/{ws_id}/teams/list");

        let response = crate::create_router(
            pool,
            crate::test_helpers::test_metrics(),
            crate::test_helpers::test_secret_key(),
        )
        .oneshot(
            Request::builder()
                .uri(&uri)
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value =
            serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
        (status, json)
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn test_list_teams_user_in_no_team(pool: PgPool) {
        let email = "alice@test.local";
        let ws_id = "ws-test";
        seed_account(&pool, email).await;
        seed_workspace_member(&pool, ws_id, email, "editor").await;
        seed_team(&pool, ws_id, "ml-team", "ML pipeline").await;

        let (status, json) = call_list_teams(pool, ws_id, email).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["items"].as_array().unwrap().len(), 0);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn test_list_teams_user_in_two_teams(pool: PgPool) {
        let email = "alice@test.local";
        let ws_id = "ws-test";
        seed_account(&pool, email).await;
        seed_workspace_member(&pool, ws_id, email, "editor").await;
        seed_team(&pool, ws_id, "backend", "Backend services").await;
        seed_team(&pool, ws_id, "ml-team", "ML pipeline").await;
        seed_team(&pool, ws_id, "data-eng", "Data ETL").await;
        seed_team_member(&pool, ws_id, email, "ml-team").await;
        seed_team_member(&pool, ws_id, email, "backend").await;

        let (status, json) = call_list_teams(pool, ws_id, email).await;
        assert_eq!(status, StatusCode::OK);
        let items = json["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["name"], "backend");
        assert_eq!(items[0]["summary"], "Backend services");
        assert!(items[0]["member_count"].as_i64().is_some());
        assert_eq!(items[1]["name"], "ml-team");
        assert_eq!(items[1]["summary"], "ML pipeline");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn test_list_teams_admin_sees_all(pool: PgPool) {
        let admin = "admin@test.local";
        let member = "member@test.local";
        let ws_id = "ws-test";
        seed_account(&pool, admin).await;
        seed_account(&pool, member).await;
        seed_workspace_member(&pool, ws_id, admin, "admin").await;
        seed_workspace_member(&pool, ws_id, member, "editor").await;
        seed_team(&pool, ws_id, "backend", "Backend services").await;
        seed_team(&pool, ws_id, "ml-team", "ML pipeline").await;
        seed_team_member(&pool, ws_id, member, "ml-team").await;

        // Admin sees all teams even if not a member
        let (status, json) = call_list_teams(pool, ws_id, admin).await;
        assert_eq!(status, StatusCode::OK);
        let items = json["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["name"], "backend");
        assert_eq!(items[0]["member_count"], 0);
        assert_eq!(items[1]["name"], "ml-team");
        assert_eq!(items[1]["member_count"], 1);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn test_list_teams_null_summary_returns_empty_string(pool: PgPool) {
        let email = "alice@test.local";
        let ws_id = "ws-test";
        seed_account(&pool, email).await;
        seed_workspace_member(&pool, ws_id, email, "admin").await;

        sqlx::query!(
            "INSERT INTO team (workspace_id, name, summary) VALUES ($1, $2, NULL)",
            ws_id,
            "ml-team",
        )
        .execute(&pool)
        .await
        .unwrap();

        let (status, json) = call_list_teams(pool, ws_id, email).await;
        assert_eq!(status, StatusCode::OK);
        let items = json["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["name"], "ml-team");
        assert_eq!(items[0]["summary"], "");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn test_list_teams_excludes_other_workspace_teams(pool: PgPool) {
        let email = "alice@test.local";
        let ws_a = "ws-a";
        let ws_b = "ws-b";
        seed_account(&pool, email).await;
        seed_workspace_member(&pool, ws_a, email, "admin").await;
        seed_workspace_member(&pool, ws_b, email, "admin").await;
        seed_team(&pool, ws_a, "team-a", "Team in workspace A").await;
        seed_team(&pool, ws_b, "team-b", "Team in workspace B").await;
        seed_team_member(&pool, ws_a, email, "team-a").await;
        seed_team_member(&pool, ws_b, email, "team-b").await;

        let (status, json) = call_list_teams(pool, ws_a, email).await;
        assert_eq!(status, StatusCode::OK);
        let items = json["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["name"], "team-a");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn test_create_team(pool: PgPool) {
        let admin = "admin@test.local";
        seed_account(&pool, admin).await;
        seed_workspace_member(&pool, "ws-1", admin, "admin").await;

        let body = serde_json::json!({ "name": "ml-team", "summary": "ML pipeline" });
        let (status, _) = call_json(
            pool.clone(),
            "POST",
            "/api/workspaces/ws-1/teams/create",
            admin,
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // Verify via list
        let (status, json) = call_list_teams(pool, "ws-1", admin).await;
        assert_eq!(status, StatusCode::OK);
        let items = json["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["name"], "ml-team");
        assert_eq!(items[0]["summary"], "ML pipeline");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn test_create_team_invalid_name(pool: PgPool) {
        let admin = "admin@test.local";
        seed_account(&pool, admin).await;
        seed_workspace_member(&pool, "ws-1", admin, "admin").await;

        let body = serde_json::json!({ "name": "Invalid Name!" });
        let (status, _) = call_json(
            pool,
            "POST",
            "/api/workspaces/ws-1/teams/create",
            admin,
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn test_create_team_duplicate(pool: PgPool) {
        let admin = "admin@test.local";
        seed_account(&pool, admin).await;
        seed_workspace_member(&pool, "ws-1", admin, "admin").await;

        let body = serde_json::json!({ "name": "ml-team" });
        let (status, _) = call_json(
            pool.clone(),
            "POST",
            "/api/workspaces/ws-1/teams/create",
            admin,
            Some(body.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, _) = call_json(
            pool,
            "POST",
            "/api/workspaces/ws-1/teams/create",
            admin,
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn test_create_team_non_admin_forbidden(pool: PgPool) {
        let editor = "editor@test.local";
        seed_account(&pool, editor).await;
        seed_workspace_member(&pool, "ws-1", editor, "editor").await;

        let body = serde_json::json!({ "name": "ml-team" });
        let (status, _) = call_json(
            pool,
            "POST",
            "/api/workspaces/ws-1/teams/create",
            editor,
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn test_delete_team(pool: PgPool) {
        let admin = "admin@test.local";
        seed_account(&pool, admin).await;
        seed_workspace_member(&pool, "ws-1", admin, "admin").await;
        seed_team(&pool, "ws-1", "ml-team", "ML pipeline").await;

        let (status, _) = call_json(
            pool.clone(),
            "DELETE",
            "/api/workspaces/ws-1/teams/delete/ml-team",
            admin,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // Verify team was deleted
        let (status, json) = call_list_teams(pool, "ws-1", admin).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["items"].as_array().unwrap().len(), 0);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn test_delete_team_not_found(pool: PgPool) {
        let admin = "admin@test.local";
        seed_account(&pool, admin).await;
        seed_workspace_member(&pool, "ws-1", admin, "admin").await;

        let (status, _) = call_json(
            pool,
            "DELETE",
            "/api/workspaces/ws-1/teams/delete/nonexistent",
            admin,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn test_team_member_crud(pool: PgPool) {
        let admin = "admin@test.local";
        let member = "member@test.local";
        seed_account(&pool, admin).await;
        seed_account(&pool, member).await;
        seed_workspace_member(&pool, "ws-1", admin, "admin").await;
        seed_workspace_member(&pool, "ws-1", member, "editor").await;
        seed_team(&pool, "ws-1", "ml-team", "ML pipeline").await;

        // Add team member as reader.
        let body = serde_json::json!({ "email": member, "role": "reader" });
        let (status, _) = call_json(
            pool.clone(),
            "POST",
            "/api/workspaces/ws-1/teams/ml-team/members",
            admin,
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // List team members → email + role.
        let (status, json) = call_json(
            pool.clone(),
            "GET",
            "/api/workspaces/ws-1/teams/ml-team/members",
            admin,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let members = json.as_array().unwrap();
        assert_eq!(members.len(), 1);
        assert_eq!(members[0]["email"], "member@test.local");
        assert_eq!(members[0]["role"], "reader");

        // Promote to writer.
        let (status, _) = call_json(
            pool.clone(),
            "PUT",
            "/api/workspaces/ws-1/teams/ml-team/members/member@test.local/role",
            admin,
            Some(serde_json::json!({ "role": "writer" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (_, json) = call_json(
            pool.clone(),
            "GET",
            "/api/workspaces/ws-1/teams/ml-team/members",
            admin,
            None,
        )
        .await;
        assert_eq!(json.as_array().unwrap()[0]["role"], "writer");

        // Remove team member
        let (status, _) = call_json(
            pool.clone(),
            "DELETE",
            "/api/workspaces/ws-1/teams/ml-team/members/member@test.local",
            admin,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // Verify member was removed
        let (status, json) = call_json(
            pool,
            "GET",
            "/api/workspaces/ws-1/teams/ml-team/members",
            admin,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json.as_array().unwrap().len(), 0);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn test_add_team_member_not_workspace_member(pool: PgPool) {
        let admin = "admin@test.local";
        let outsider = "outsider@test.local";
        seed_account(&pool, admin).await;
        seed_account(&pool, outsider).await;
        seed_workspace_member(&pool, "ws-1", admin, "admin").await;
        seed_team(&pool, "ws-1", "ml-team", "ML pipeline").await;

        let body = serde_json::json!({ "email": outsider });
        let (status, _) = call_json(
            pool,
            "POST",
            "/api/workspaces/ws-1/teams/ml-team/members",
            admin,
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn test_add_team_member_duplicate(pool: PgPool) {
        let admin = "admin@test.local";
        let member = "member@test.local";
        seed_account(&pool, admin).await;
        seed_account(&pool, member).await;
        seed_workspace_member(&pool, "ws-1", admin, "admin").await;
        seed_workspace_member(&pool, "ws-1", member, "editor").await;
        seed_team(&pool, "ws-1", "ml-team", "ML pipeline").await;
        seed_team_member(&pool, "ws-1", member, "ml-team").await;

        let body = serde_json::json!({ "email": member });
        let (status, _) = call_json(
            pool,
            "POST",
            "/api/workspaces/ws-1/teams/ml-team/members",
            admin,
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn test_team_quota_crud(pool: PgPool) {
        let admin = "admin@test.local";
        seed_account(&pool, admin).await;
        seed_workspace_member(&pool, "ws-1", admin, "admin").await;
        seed_team(&pool, "ws-1", "ml-team", "ML pipeline").await;

        // Get quota (no quota set yet, should return all nulls)
        let (status, json) = call_json(
            pool.clone(),
            "GET",
            "/api/workspaces/ws-1/teams/ml-team/quota",
            admin,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(json["max_concurrent_runs"].is_null());
        assert!(json["max_cpus"].is_null());

        // Update quota
        let body = serde_json::json!({
            "max_concurrent_runs": 4,
            "max_cpus": 8.0,
            "max_memory_mb": 16384,
            "max_daily_runs": 1000,
            "max_storage_bytes": null,
            "max_run_timeout_secs": 3600
        });
        let (status, _) = call_json(
            pool.clone(),
            "PUT",
            "/api/workspaces/ws-1/teams/ml-team/quota",
            admin,
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // Get quota again to verify
        let (status, json) = call_json(
            pool.clone(),
            "GET",
            "/api/workspaces/ws-1/teams/ml-team/quota",
            admin,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["max_concurrent_runs"], 4);
        assert_eq!(json["max_daily_runs"], 1000);
        assert!(json["max_storage_bytes"].is_null());
        assert_eq!(json["max_run_timeout_secs"], 3600);

        // Update quota again (upsert)
        let body = serde_json::json!({
            "max_concurrent_runs": 8,
            "max_cpus": null,
            "max_memory_mb": null,
            "max_daily_runs": null,
            "max_storage_bytes": null,
            "max_run_timeout_secs": null
        });
        let (status, _) = call_json(
            pool.clone(),
            "PUT",
            "/api/workspaces/ws-1/teams/ml-team/quota",
            admin,
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // Verify update
        let (status, json) = call_json(
            pool,
            "GET",
            "/api/workspaces/ws-1/teams/ml-team/quota",
            admin,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["max_concurrent_runs"], 8);
        assert!(json["max_cpus"].is_null());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn test_update_quota_team_not_found(pool: PgPool) {
        let admin = "admin@test.local";
        seed_account(&pool, admin).await;
        seed_workspace_member(&pool, "ws-1", admin, "admin").await;

        let body = serde_json::json!({
            "max_concurrent_runs": 4,
            "max_cpus": null,
            "max_memory_mb": null,
            "max_daily_runs": null,
            "max_storage_bytes": null,
            "max_run_timeout_secs": null
        });
        let (status, _) = call_json(
            pool,
            "PUT",
            "/api/workspaces/ws-1/teams/nonexistent/quota",
            admin,
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn test_full_team_lifecycle(pool: PgPool) {
        let admin = "admin@test.local";
        let member = "member@test.local";
        seed_account(&pool, admin).await;
        seed_account(&pool, member).await;
        seed_workspace_member(&pool, "ws-1", admin, "admin").await;
        seed_workspace_member(&pool, "ws-1", member, "editor").await;

        // Create team
        let body = serde_json::json!({ "name": "data-eng", "summary": "Data Engineering" });
        let (status, _) = call_json(
            pool.clone(),
            "POST",
            "/api/workspaces/ws-1/teams/create",
            admin,
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // Add member
        let body = serde_json::json!({ "email": member });
        let (status, _) = call_json(
            pool.clone(),
            "POST",
            "/api/workspaces/ws-1/teams/data-eng/members",
            admin,
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // Set quota
        let body = serde_json::json!({
            "max_concurrent_runs": 2,
            "max_cpus": 4.0,
            "max_memory_mb": 8192,
            "max_daily_runs": 500,
            "max_storage_bytes": null,
            "max_run_timeout_secs": 1800
        });
        let (status, _) = call_json(
            pool.clone(),
            "PUT",
            "/api/workspaces/ws-1/teams/data-eng/quota",
            admin,
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // Remove member
        let (status, _) = call_json(
            pool.clone(),
            "DELETE",
            "/api/workspaces/ws-1/teams/data-eng/members/member@test.local",
            admin,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // Delete team (CASCADE handles team_member, team_quota)
        let (status, _) = call_json(
            pool.clone(),
            "DELETE",
            "/api/workspaces/ws-1/teams/delete/data-eng",
            admin,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // Verify team is gone
        let (status, json) = call_list_teams(pool, "ws-1", admin).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["items"].as_array().unwrap().len(), 0);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn test_get_team(pool: PgPool) {
        let admin = "admin@test.local";
        let member = "member@test.local";
        seed_account(&pool, admin).await;
        seed_account(&pool, member).await;
        seed_workspace_member(&pool, "ws-1", admin, "admin").await;
        seed_workspace_member(&pool, "ws-1", member, "editor").await;
        seed_team(&pool, "ws-1", "ml-team", "ML pipeline").await;
        seed_team_member(&pool, "ws-1", member, "ml-team").await;

        let (status, json) = call_json(
            pool,
            "GET",
            "/api/workspaces/ws-1/teams/get/ml-team",
            admin,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["name"], "ml-team");
        assert_eq!(json["summary"], "ML pipeline");
        assert_eq!(json["member_count"], 1);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn test_get_team_not_found(pool: PgPool) {
        let admin = "admin@test.local";
        seed_account(&pool, admin).await;
        seed_workspace_member(&pool, "ws-1", admin, "admin").await;

        let (status, _) = call_json(
            pool,
            "GET",
            "/api/workspaces/ws-1/teams/get/nonexistent",
            admin,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn test_get_team_non_admin_forbidden(pool: PgPool) {
        let editor = "editor@test.local";
        seed_account(&pool, editor).await;
        seed_workspace_member(&pool, "ws-1", editor, "editor").await;
        seed_team(&pool, "ws-1", "ml-team", "ML pipeline").await;

        let (status, _) = call_json(
            pool,
            "GET",
            "/api/workspaces/ws-1/teams/get/ml-team",
            editor,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }
}
