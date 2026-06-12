use axum::Json;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use sqlx::PgPool;

use crate::error::ApiError;

#[derive(Debug, serde::Deserialize)]
pub struct UserSearchQuery {
    pub q: String,
}

#[derive(serde::Serialize)]
pub struct UserSearchItem {
    pub email: String,
}

#[tracing::instrument(name = "api::search_users", skip(db, headers))]
pub async fn search_users(
    State(db): State<PgPool>,
    headers: HeaderMap,
    Query(query): Query<UserSearchQuery>,
) -> Result<Json<Vec<UserSearchItem>>, ApiError> {
    let email = crate::auth::email_from_bearer_headers(&headers)?;

    if query.q.len() < 2 {
        return Err(ApiError::BadRequest(
            "query must be at least 2 characters".into(),
        ));
    }

    // Verify caller is admin in at least one workspace
    let is_admin = sqlx::query_scalar!(
        r#"SELECT EXISTS(
            SELECT 1 FROM workspace_member
            WHERE email = $1 AND role = 'admin'
        ) AS "exists!""#,
        email,
    )
    .fetch_one(&db)
    .await?;

    if !is_admin {
        return Err(ApiError::Forbidden("admin role required".into()));
    }

    let escaped = query.q.replace('%', "\\%").replace('_', "\\_");
    let pattern = format!("%{escaped}%");
    let items = sqlx::query_as!(
        UserSearchItem,
        r#"SELECT email
           FROM account
           WHERE email ILIKE $1
           ORDER BY email ASC
           LIMIT 20"#,
        pattern,
    )
    .fetch_all(&db)
    .await?;

    Ok(Json(items))
}

#[cfg(test)]
mod tests {
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use sqlx::PgPool;
    use tower::ServiceExt;

    use crate::test_helpers::*;

    #[sqlx::test(migrations = "../../migrations")]
    async fn test_search_users_success(pool: PgPool) {
        let admin = "admin@test.local";
        seed_account(&pool, admin).await;
        seed_workspace_member(&pool, "ws-1", admin, "admin").await;

        // Create some searchable users
        seed_account(&pool, "alice@test.local").await;
        seed_account(&pool, "alicia@test.local").await;
        seed_account(&pool, "bob@test.local").await;

        let token = valid_jwt(admin);
        let response = crate::create_router(
            pool,
            crate::test_helpers::test_metrics(),
            crate::test_helpers::test_secret_key(),
        )
        .oneshot(
            Request::builder()
                .uri("/api/users/search?q=ali")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let items = json.as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["email"], "alice@test.local");
        assert_eq!(items[1]["email"], "alicia@test.local");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn test_search_users_query_too_short(pool: PgPool) {
        let admin = "admin@test.local";
        seed_account(&pool, admin).await;
        seed_workspace_member(&pool, "ws-1", admin, "admin").await;

        let token = valid_jwt(admin);
        let response = crate::create_router(
            pool,
            crate::test_helpers::test_metrics(),
            crate::test_helpers::test_secret_key(),
        )
        .oneshot(
            Request::builder()
                .uri("/api/users/search?q=a")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn test_search_users_non_admin_forbidden(pool: PgPool) {
        let editor = "editor@test.local";
        seed_account(&pool, editor).await;
        seed_workspace_member(&pool, "ws-1", editor, "editor").await;

        let token = valid_jwt(editor);
        let response = crate::create_router(
            pool,
            crate::test_helpers::test_metrics(),
            crate::test_helpers::test_secret_key(),
        )
        .oneshot(
            Request::builder()
                .uri("/api/users/search?q=test")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn test_search_users_no_auth(pool: PgPool) {
        let response = crate::create_router(
            pool,
            crate::test_helpers::test_metrics(),
            crate::test_helpers::test_secret_key(),
        )
        .oneshot(
            Request::builder()
                .uri("/api/users/search?q=test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn test_search_users_limit_20(pool: PgPool) {
        let admin = "admin@test.local";
        seed_account(&pool, admin).await;
        seed_workspace_member(&pool, "ws-1", admin, "admin").await;

        // Create 25 users matching the query
        for i in 0..25 {
            seed_account(&pool, &format!("user{i:02}@test.local")).await;
        }

        let token = valid_jwt(admin);
        let response = crate::create_router(
            pool,
            crate::test_helpers::test_metrics(),
            crate::test_helpers::test_secret_key(),
        )
        .oneshot(
            Request::builder()
                .uri("/api/users/search?q=user")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json.as_array().unwrap().len(), 20);
    }
}
