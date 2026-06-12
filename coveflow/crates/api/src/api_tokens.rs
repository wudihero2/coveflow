//! Personal API Token (PAT) CRUD.
//!
//! Top-level (non-workspace-scoped) routes. The caller manages **their own**
//! tokens, authenticated by JWT (`email_from_bearer_headers`). A token is shown
//! once in the create response and can be revealed later (it is stored encrypted
//! + hashed). v1 PATs authenticate only inbound webhook calls.
//!
//! SECURITY: the plaintext token appears only in create / reveal responses,
//! never in logs or trace fields.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use coveflow_types::crypto::{self, SecretKey};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::email_from_bearer_headers;
use crate::error::ApiError;

/// Token metadata — never includes the token value.
#[derive(Serialize)]
pub struct ApiTokenListItem {
    pub id: Uuid,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Deserialize)]
pub struct CreateTokenRequest {
    pub name: String,
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
}

/// Create response — the only place (besides reveal) the plaintext token appears.
#[derive(Serialize)]
pub struct ApiTokenCreated {
    pub id: Uuid,
    pub name: String,
    pub token: String,
    pub expires_at: Option<DateTime<Utc>>,
}

#[tracing::instrument(name = "api::list_tokens", skip(db, headers))]
pub async fn list_tokens(
    State(db): State<PgPool>,
    headers: HeaderMap,
) -> Result<Json<Vec<ApiTokenListItem>>, ApiError> {
    let email = email_from_bearer_headers(&headers)?;
    let rows = sqlx::query!(
        "SELECT id, name, created_at, last_used_at, expires_at
         FROM api_token WHERE email = $1 ORDER BY created_at DESC",
        email
    )
    .fetch_all(&db)
    .await?;

    let items = rows
        .into_iter()
        .map(|r| ApiTokenListItem {
            id: r.id,
            name: r.name,
            created_at: r.created_at,
            last_used_at: r.last_used_at,
            expires_at: r.expires_at,
        })
        .collect();
    Ok(Json(items))
}

#[tracing::instrument(name = "api::create_token", skip(db, key, headers, req), fields(name = %req.name))]
pub async fn create_token(
    State(db): State<PgPool>,
    State(key): State<SecretKey>,
    headers: HeaderMap,
    Json(req): Json<CreateTokenRequest>,
) -> Result<Response, ApiError> {
    let email = email_from_bearer_headers(&headers)?;
    if req.name.trim().is_empty() {
        return Err(ApiError::BadRequest("name must not be empty".into()));
    }
    if matches!(req.expires_at, Some(exp) if exp <= Utc::now()) {
        return Err(ApiError::BadRequest(
            "expires_at must be in the future".into(),
        ));
    }

    let token = coveflow_types::api_token::generate_token();
    let hash = coveflow_types::api_token::token_hash(&token);
    let encrypted =
        crypto::encrypt(&key, token.as_bytes()).map_err(|e| ApiError::Internal(e.to_string()))?;

    let id = sqlx::query_scalar!(
        "INSERT INTO api_token (email, name, token_hash, token_encrypted, expires_at)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (email, name) DO NOTHING
         RETURNING id",
        email,
        req.name,
        hash,
        encrypted,
        req.expires_at,
    )
    .fetch_optional(&db)
    .await?
    .ok_or_else(|| ApiError::Conflict(format!("token '{}' already exists", req.name)))?;

    Ok((
        StatusCode::CREATED,
        Json(ApiTokenCreated {
            id,
            name: req.name,
            token,
            expires_at: req.expires_at,
        }),
    )
        .into_response())
}

#[derive(Serialize)]
pub struct RevealedToken {
    pub token: String,
}

#[tracing::instrument(name = "api::reveal_token", skip(db, key, headers), fields(%id))]
pub async fn reveal_token(
    State(db): State<PgPool>,
    State(key): State<SecretKey>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<RevealedToken>, ApiError> {
    let email = email_from_bearer_headers(&headers)?;
    let blob = sqlx::query_scalar!(
        "SELECT token_encrypted FROM api_token WHERE id = $1 AND email = $2",
        id,
        email
    )
    .fetch_optional(&db)
    .await?
    .ok_or(ApiError::NotFound)?;

    let plaintext = crypto::decrypt(&key, &blob).map_err(|e| ApiError::Internal(e.to_string()))?;
    let token = String::from_utf8(plaintext)
        .map_err(|_| ApiError::Internal("token is not valid UTF-8".into()))?;
    Ok(Json(RevealedToken { token }))
}

#[tracing::instrument(name = "api::revoke_token", skip(db, headers), fields(%id))]
pub async fn revoke_token(
    State(db): State<PgPool>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let email = email_from_bearer_headers(&headers)?;
    let res = sqlx::query!(
        "DELETE FROM api_token WHERE id = $1 AND email = $2",
        id,
        email
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
    use crate::test_helpers::{call_json, seed_account};
    use axum::http::StatusCode;
    use sqlx::PgPool;

    #[sqlx::test(migrations = "../../migrations")]
    async fn create_list_reveal_revoke(pool: PgPool) {
        seed_account(&pool, "a@x.com").await;

        // Create → one-time plaintext token.
        let (st, body) = call_json(
            pool.clone(),
            "POST",
            "/api/account/tokens",
            "a@x.com",
            Some(serde_json::json!({ "name": "ci" })),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED);
        let token = body["token"].as_str().unwrap().to_string();
        let id = body["id"].as_str().unwrap();
        assert!(token.starts_with("cf_pat_"));

        // DB stores no plaintext.
        let stored: Vec<u8> =
            sqlx::query_scalar!("SELECT token_encrypted FROM api_token WHERE email = 'a@x.com'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(!stored.windows(7).any(|w| w == b"cf_pat_"));

        // List = metadata only, no value.
        let (st, body) =
            call_json(pool.clone(), "GET", "/api/account/tokens", "a@x.com", None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body.as_array().unwrap().len(), 1);
        assert!(body[0].get("token").is_none(), "list must not expose token");

        // Reveal returns the same plaintext.
        let (st, body) = call_json(
            pool.clone(),
            "GET",
            &format!("/api/account/tokens/{id}/reveal"),
            "a@x.com",
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body["token"], token);

        // Revoke → then reveal 404.
        let (st, _) = call_json(
            pool.clone(),
            "DELETE",
            &format!("/api/account/tokens/{id}"),
            "a@x.com",
            None,
        )
        .await;
        assert_eq!(st, StatusCode::NO_CONTENT);
        let (st, _) = call_json(
            pool.clone(),
            "GET",
            &format!("/api/account/tokens/{id}/reveal"),
            "a@x.com",
            None,
        )
        .await;
        assert_eq!(st, StatusCode::NOT_FOUND);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn duplicate_name_is_409_and_others_hidden(pool: PgPool) {
        seed_account(&pool, "a@x.com").await;
        seed_account(&pool, "b@x.com").await;
        let body = Some(serde_json::json!({ "name": "dup" }));
        let (st, created) = call_json(
            pool.clone(),
            "POST",
            "/api/account/tokens",
            "a@x.com",
            body.clone(),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED);
        let (st, _) = call_json(pool.clone(), "POST", "/api/account/tokens", "a@x.com", body).await;
        assert_eq!(st, StatusCode::CONFLICT);

        // b cannot reveal a's token.
        let id = created["id"].as_str().unwrap();
        let (st, _) = call_json(
            pool.clone(),
            "GET",
            &format!("/api/account/tokens/{id}/reveal"),
            "b@x.com",
            None,
        )
        .await;
        assert_eq!(st, StatusCode::NOT_FOUND);
    }
}
