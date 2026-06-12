//! Inbound webhook trigger endpoint.
//!
//! PUBLIC route `POST /api/webhooks/{trigger_id}` — external systems call it with
//! `Authorization: Bearer <PAT>`. The PAT resolves to a user who must have
//! `can_write` on the flow; the run executes as that user (async). The JSON body
//! becomes `flow.input`; request metadata is recorded as provenance →
//! `ctx.trigger`.
//!
//! SECURITY: never logs the PAT, the Authorization header, or cookies.

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use coveflow_queue::{TriggerError, submit_triggered_run};
use coveflow_types::trigger::{TriggerRow, WEBHOOK_TYPE};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::{authed_user_for, email_from_pat};
use crate::error::ApiError;
use crate::schedules::current_flow_path;

#[tracing::instrument(
    name = "api::webhook_fire",
    skip(db, headers, body),
    fields(%trigger_id, workspace_id = tracing::field::Empty, run_id = tracing::field::Empty)
)]
pub async fn fire_webhook(
    State(db): State<PgPool>,
    Path(trigger_id): Path<Uuid>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    // 1. Resolve an enabled webhook trigger. Unknown/disabled → 404 (don't leak
    //    existence).
    let trigger = load_enabled_webhook(&db, trigger_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    tracing::Span::current().record("workspace_id", trigger.workspace_id.as_str());

    // 2. Authenticate the caller via their PAT, then authorize against the flow.
    let email = email_from_pat(&db, &headers).await?;
    let user = authed_user_for(&db, email.clone(), trigger.workspace_id.clone()).await?;
    let flow_path = current_flow_path(&db, &trigger.workspace_id, trigger.flow_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    user.require_writer(&flow_path)?;

    // 3. Body → flow.input. Empty body = {}; non-JSON → 400.
    let input = parse_input(&body)?;

    // 4. Provenance (no secrets: never the Authorization/cookie headers).
    let provenance = build_provenance(&trigger, &headers);

    // 5. Submit asynchronously as the caller.
    let run_id = submit_triggered_run(&db, &trigger, &email, input, provenance)
        .await
        .map_err(map_trigger_error)?;
    tracing::Span::current().record("run_id", run_id.to_string().as_str());

    let status_url = format!("/api/workspaces/{}/runs/get/{run_id}", trigger.workspace_id);
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({ "run_id": run_id, "status_url": status_url })),
    )
        .into_response())
}

async fn load_enabled_webhook(db: &PgPool, id: Uuid) -> Result<Option<TriggerRow>, ApiError> {
    let row = sqlx::query!(
        "SELECT id, workspace_id, flow_id, trigger_type, name, enabled, config, created_by
         FROM trigger WHERE id = $1 AND trigger_type = $2 AND enabled = TRUE",
        id,
        WEBHOOK_TYPE
    )
    .fetch_optional(db)
    .await?;
    Ok(row.map(|r| TriggerRow {
        id: r.id,
        workspace_id: r.workspace_id,
        flow_id: r.flow_id,
        trigger_type: r.trigger_type,
        name: r.name,
        enabled: r.enabled,
        config: r.config,
        created_by: r.created_by,
    }))
}

fn parse_input(body: &Bytes) -> Result<Value, ApiError> {
    if body.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_slice(body)
        .map_err(|_| ApiError::BadRequest("request body must be valid JSON".into()))
}

/// Header-summary provenance for `ctx.trigger`. Deliberately excludes the
/// Authorization header, cookies, and any token material.
fn build_provenance(trigger: &TriggerRow, headers: &HeaderMap) -> Value {
    let header_str = |name: &str| -> Option<String> {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(String::from)
    };
    // Best-effort caller IP from proxy headers (no ConnectInfo plumbing in v1).
    let source_ip = header_str("x-forwarded-for")
        .map(|v| v.split(',').next().unwrap_or("").trim().to_string())
        .or_else(|| header_str("x-real-ip"));

    json!({
        "type": WEBHOOK_TYPE,
        "trigger_id": trigger.id,
        "trigger_name": trigger.name,
        "method": "POST",
        "source_ip": source_ip,
        "user_agent": header_str("user-agent"),
        "content_type": header_str("content-type"),
        "received_at": chrono::Utc::now().to_rfc3339(),
    })
}

fn map_trigger_error(e: TriggerError) -> ApiError {
    match e {
        TriggerError::MaxActiveRuns(name) => {
            ApiError::TooManyRequests(format!("trigger '{name}' is at its max active runs"))
        }
        TriggerError::FlowNotFound(_) => ApiError::NotFound,
        TriggerError::InvalidConfig(m) => ApiError::BadRequest(m),
        TriggerError::Queue(q) => ApiError::Internal(q.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use crate::test_helpers::{
        call_raw, seed_account, seed_api_token, seed_flow, seed_workspace_member,
    };
    use axum::http::StatusCode;
    use sqlx::PgPool;
    use uuid::Uuid;

    async fn member(db: &PgPool, ws: &str, email: &str, role: &str) {
        seed_account(db, email).await;
        seed_workspace_member(db, ws, email, role).await;
    }

    async fn insert_webhook(db: &PgPool, ws: &str, flow_id: Uuid, enabled: bool) -> Uuid {
        sqlx::query_scalar!(
            "INSERT INTO trigger (workspace_id, flow_id, trigger_type, name, enabled, created_by)
             VALUES ($1, $2, 'webhook', 'hook', $3, 'a@x.com') RETURNING id",
            ws,
            flow_id,
            enabled
        )
        .fetch_one(db)
        .await
        .unwrap()
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn fires_run_as_caller(pool: PgPool) {
        member(&pool, "ws1", "a@x.com", "admin").await;
        let flow_id = seed_flow(&pool, "ws1", "workspace/f").await;
        let trig = insert_webhook(&pool, "ws1", flow_id, true).await;
        let token = seed_api_token(&pool, "a@x.com").await;

        let (st, body) = call_raw(
            pool.clone(),
            "POST",
            &format!("/api/webhooks/{trig}"),
            &format!("Bearer {token}"),
            Some("application/json"),
            br#"{"x":1}"#.to_vec(),
        )
        .await;
        assert_eq!(st, StatusCode::ACCEPTED);
        let run_id = body["run_id"].as_str().unwrap();

        let row = sqlx::query!(
            "SELECT created_by, trigger_id, args, kind FROM run WHERE id = $1::uuid",
            run_id.parse::<Uuid>().unwrap()
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.created_by, "a@x.com");
        assert_eq!(row.trigger_id, Some(trig));
        assert_eq!(row.kind, "flow");
        assert_eq!(row.args, Some(serde_json::json!({ "x": 1 })));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn disabled_trigger_is_404(pool: PgPool) {
        member(&pool, "ws1", "a@x.com", "admin").await;
        let flow_id = seed_flow(&pool, "ws1", "workspace/f").await;
        let trig = insert_webhook(&pool, "ws1", flow_id, false).await;
        let token = seed_api_token(&pool, "a@x.com").await;
        let (st, _) = call_raw(
            pool.clone(),
            "POST",
            &format!("/api/webhooks/{trig}"),
            &format!("Bearer {token}"),
            Some("application/json"),
            b"{}".to_vec(),
        )
        .await;
        assert_eq!(st, StatusCode::NOT_FOUND);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn bad_token_is_401(pool: PgPool) {
        member(&pool, "ws1", "a@x.com", "admin").await;
        let flow_id = seed_flow(&pool, "ws1", "workspace/f").await;
        let trig = insert_webhook(&pool, "ws1", flow_id, true).await;
        let (st, _) = call_raw(
            pool.clone(),
            "POST",
            &format!("/api/webhooks/{trig}"),
            "Bearer cf_pat_bogus",
            Some("application/json"),
            b"{}".to_vec(),
        )
        .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn no_write_permission_is_403(pool: PgPool) {
        member(&pool, "ws1", "a@x.com", "admin").await;
        member(&pool, "ws1", "v@x.com", "viewer").await;
        let flow_id = seed_flow(&pool, "ws1", "workspace/f").await;
        let trig = insert_webhook(&pool, "ws1", flow_id, true).await;
        let token = seed_api_token(&pool, "v@x.com").await; // viewer can't write workspace/
        let (st, _) = call_raw(
            pool.clone(),
            "POST",
            &format!("/api/webhooks/{trig}"),
            &format!("Bearer {token}"),
            Some("application/json"),
            b"{}".to_vec(),
        )
        .await;
        assert_eq!(st, StatusCode::FORBIDDEN);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn non_json_body_is_400(pool: PgPool) {
        member(&pool, "ws1", "a@x.com", "admin").await;
        let flow_id = seed_flow(&pool, "ws1", "workspace/f").await;
        let trig = insert_webhook(&pool, "ws1", flow_id, true).await;
        let token = seed_api_token(&pool, "a@x.com").await;
        let (st, _) = call_raw(
            pool.clone(),
            "POST",
            &format!("/api/webhooks/{trig}"),
            &format!("Bearer {token}"),
            Some("application/json"),
            b"not json".to_vec(),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
    }
}
