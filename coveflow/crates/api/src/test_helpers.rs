#![cfg(test)]

use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use sqlx::PgPool;
use tower::ServiceExt;

use crate::ApiMetrics;
use crate::auth;

pub fn test_metrics() -> Arc<ApiMetrics> {
    let mut registry = prometheus_client::registry::Registry::default();
    Arc::new(ApiMetrics::register(&mut registry))
}

/// A deterministic, valid 32-byte secret-store key for router tests.
pub fn test_secret_key() -> coveflow_types::crypto::SecretKey {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode([7u8; 32]);
    coveflow_types::crypto::SecretKey::from_base64(&b64).expect("valid test key")
}

pub fn valid_jwt(email: &str) -> String {
    auth::generate_access_token(email).expect("generate JWT")
}

pub async fn seed_account(db: &PgPool, email: &str) {
    let hash = "$argon2id$v=19$m=19456,t=2,p=1$AAAAAAAAAAAAAAAAAAAAAA$eFBpFPJyjimZN8tCAsTuRgNxNlFxnWF/5o2sow4q3BA";
    sqlx::query!(
        "INSERT INTO account (email, password_hash) VALUES ($1, $2)",
        email,
        hash
    )
    .execute(db)
    .await
    .unwrap();
}

/// Seed an account flagged as an instance admin (`account.is_admin = true`).
pub async fn seed_instance_admin(db: &PgPool, email: &str) {
    seed_account(db, email).await;
    sqlx::query!("UPDATE account SET is_admin = TRUE WHERE email = $1", email)
        .execute(db)
        .await
        .unwrap();
}

pub async fn seed_workspace_member(db: &PgPool, ws_id: &str, email: &str, role: &str) {
    sqlx::query!(
        "INSERT INTO workspace (id, name, owner) VALUES ($1, $2, $3)
         ON CONFLICT (id) DO NOTHING",
        ws_id,
        "Test Workspace",
        email
    )
    .execute(db)
    .await
    .unwrap();

    sqlx::query!(
        "INSERT INTO workspace_member (workspace_id, email, role) VALUES ($1, $2, $3)",
        ws_id,
        email,
        role
    )
    .execute(db)
    .await
    .unwrap();
}

pub async fn seed_team(db: &PgPool, ws_id: &str, team_name: &str, summary: &str) {
    sqlx::query!(
        "INSERT INTO team (workspace_id, name, summary) VALUES ($1, $2, $3)",
        ws_id,
        team_name,
        summary
    )
    .execute(db)
    .await
    .unwrap();
}

pub async fn seed_team_member(db: &PgPool, ws_id: &str, email: &str, team_name: &str) {
    sqlx::query!(
        "INSERT INTO team_member (workspace_id, email, team_name) VALUES ($1, $2, $3)",
        ws_id,
        email,
        team_name
    )
    .execute(db)
    .await
    .unwrap();
}

pub async fn call_json(
    pool: PgPool,
    method: &str,
    uri: &str,
    email: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let token = valid_jwt(email);
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("Authorization", format!("Bearer {token}"));

    let request_body = if let Some(json) = body {
        builder = builder.header("Content-Type", "application/json");
        Body::from(serde_json::to_vec(&json).unwrap())
    } else {
        Body::empty()
    };

    let response =
        crate::create_router(pool, test_metrics(), crate::test_helpers::test_secret_key())
            .oneshot(builder.body(request_body).unwrap())
            .await
            .unwrap();

    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

/// Raw request with an explicit `Authorization` value (e.g. a PAT, not a JWT) and
/// raw body bytes — for the public webhook endpoint.
pub async fn call_raw(
    pool: PgPool,
    method: &str,
    uri: &str,
    authorization: &str,
    content_type: Option<&str>,
    body: Vec<u8>,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("Authorization", authorization);
    if let Some(ct) = content_type {
        builder = builder.header("Content-Type", ct);
    }
    let response = crate::create_router(pool, test_metrics(), test_secret_key())
        .oneshot(builder.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

/// Seed a flow revision and return its stable `flow_id`.
pub async fn seed_flow(db: &PgPool, ws: &str, path: &str) -> uuid::Uuid {
    let flow_id = uuid::Uuid::new_v4();
    sqlx::query!(
        "INSERT INTO flow (workspace_id, path, revision, summary, value, edited_by, flow_id)
         VALUES ($1, $2, 1, '', $3, 'u@test', $4)",
        ws,
        path,
        serde_json::json!({ "nodes": [], "edges": [] }),
        flow_id
    )
    .execute(db)
    .await
    .unwrap();
    flow_id
}

/// Insert an API token for `email`, returning the plaintext (for Bearer use).
pub async fn seed_api_token(db: &PgPool, email: &str) -> String {
    let token = coveflow_types::api_token::generate_token();
    let hash = coveflow_types::api_token::token_hash(&token);
    sqlx::query!(
        "INSERT INTO api_token (email, name, token_hash, token_encrypted)
         VALUES ($1, 't', $2, $3)",
        email,
        hash,
        vec![0u8; 16]
    )
    .execute(db)
    .await
    .unwrap();
    token
}
