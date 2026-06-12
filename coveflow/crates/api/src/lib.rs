pub mod api_tokens;
pub mod auth;
pub mod cluster;
pub(crate) mod common;
pub mod error;
pub mod flows;
mod health;
pub mod members;
pub mod runs;
pub mod schedules;
pub mod script_schema;
pub mod scripts;
pub mod secrets;
pub mod services;
pub mod teams;
#[cfg(test)]
pub(crate) mod test_helpers;
pub mod triggers;
pub mod users;
pub mod webhooks;
pub mod workspaces;

use std::sync::Arc;

use axum::Router;
use axum::extract::{FromRef, Request, State};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{delete, get, post, put};
use coveflow_types::crypto::SecretKey;
use prometheus_client::encoding::EncodeLabelSet;
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::histogram::Histogram;
use sqlx::PgPool;
use tracing::Instrument;

/// Router state. `FromRef` lets handlers extract just the piece they need —
/// existing handlers keep using `State<PgPool>`, secret handlers add
/// `State<SecretKey>` — so no handler signature churns over adding the key.
#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub secret_key: SecretKey,
}

impl FromRef<AppState> for PgPool {
    fn from_ref(state: &AppState) -> Self {
        state.db.clone()
    }
}

impl FromRef<AppState> for SecretKey {
    fn from_ref(state: &AppState) -> Self {
        state.secret_key.clone()
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct HttpLabels {
    pub method: String,
    pub path: String,
    pub status: u16,
}

#[derive(Clone)]
pub struct ApiMetrics {
    pub http_requests_total: Family<HttpLabels, Counter>,
    pub http_request_duration_seconds: Histogram,
}

impl ApiMetrics {
    pub fn register(registry: &mut prometheus_client::registry::Registry) -> Self {
        let http_requests_total = Family::<HttpLabels, Counter>::default();
        registry.register(
            "http_requests",
            "Total number of HTTP requests",
            http_requests_total.clone(),
        );

        let duration_buckets: Vec<f64> =
            prometheus_client::metrics::histogram::exponential_buckets(0.005, 2.0, 16).collect();
        let http_request_duration_seconds = Histogram::new(duration_buckets);
        registry.register(
            "http_request_duration_seconds",
            "HTTP request duration in seconds",
            http_request_duration_seconds.clone(),
        );

        Self {
            http_requests_total,
            http_request_duration_seconds,
        }
    }
}

/// Normalize a request path to a route template, replacing dynamic segments
/// (UUIDs, emails, team names) with placeholders. This keeps the
/// Prometheus `path` label cardinality bounded.
fn normalize_path(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').collect();
    let mut out = Vec::with_capacity(parts.len());
    let mut i = 0;
    while i < parts.len() {
        let seg = parts[i];
        let prev = if i > 0 {
            parts.get(i - 1).copied()
        } else {
            None
        };

        // /api/workspaces/{workspace_id}/...
        if prev == Some("workspaces") && !seg.is_empty() && seg != "workspaces" {
            out.push(":ws");
            i += 1;
            continue;
        }
        // /runs/get/{run_id}, /scripts/delete/{id}, etc.
        if matches!(seg, "get" | "delete") {
            out.push(seg);
            if i + 1 < parts.len() {
                out.push(":id");
                i += 2;
                continue;
            }
        }
        // /members/{email}, /teams/{name}/...
        if matches!(prev, Some("members" | "teams"))
            && !matches!(
                seg,
                "list" | "create" | "delete" | "members" | "quota" | "get"
            )
        {
            out.push(":id");
            i += 1;
            continue;
        }
        // /acl/{subject}
        // UUID-like segments (32+ hex chars with dashes)
        if seg.len() >= 32 && seg.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
            out.push(":id");
            i += 1;
            continue;
        }
        // /scripts/get/path/... or /scripts/history/... — catch-all
        if matches!(seg, "path" | "history") {
            out.push(seg);
            out.push(":path");
            break;
        }
        out.push(seg);
        i += 1;
    }
    out.join("/")
}

async fn request_trace(
    State(metrics): State<Arc<ApiMetrics>>,
    req: Request,
    next: Next,
) -> Response {
    let method = req.method().clone();
    let uri = req.uri().clone();
    let path = uri.path().to_owned();
    let skip = path.contains("/services/logs");

    let span = tracing::info_span!(
        "request",
        %method,
        uri = %uri,
        db_log_skip = skip,
    );

    async move {
        let start = std::time::Instant::now();
        let response = next.run(req).await;
        let elapsed = start.elapsed();
        let status = response.status().as_u16();

        let latency_ms = elapsed.as_millis() as u64;
        tracing::info!(status, latency_ms, "{method} {path}");

        let labels = HttpLabels {
            method: method.to_string(),
            path: normalize_path(&path),
            status,
        };
        metrics.http_requests_total.get_or_create(&labels).inc();
        metrics
            .http_request_duration_seconds
            .observe(elapsed.as_secs_f64());

        response
    }
    .instrument(span)
    .await
}

pub fn create_router(db: PgPool, metrics: Arc<ApiMetrics>, secret_key: SecretKey) -> Router {
    let auth_middleware = axum::middleware::from_fn_with_state(db.clone(), auth::require_auth);

    Router::new()
        // Public routes (no auth required)
        .route("/health", get(health::health))
        .route("/api/auth/login", post(auth::login))
        .route("/api/auth/signup", post(auth::signup))
        .route("/api/auth/refresh", post(auth::refresh))
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/workspaces", get(workspaces::list_workspaces))
        // Global authenticated route — NOT workspace-scoped, so it cannot
        // use the workspace auth middleware (which requires {workspace_id} in
        // the path). The handler authenticates via email_from_bearer_headers()
        // and checks admin status itself.
        .route("/api/users/search", get(users::search_users))
        // Cluster dashboard — cross-workspace, instance-admin only. Like
        // /api/users/search these are NOT workspace-scoped, so the handlers
        // authenticate themselves rather than using the workspace middleware.
        .route("/api/admin/cluster/summary", get(cluster::cluster_summary))
        .route("/api/admin/cluster/workers", get(cluster::cluster_workers))
        .route(
            "/api/admin/cluster/workers/{worker}/runs",
            get(cluster::cluster_worker_runs),
        )
        // Personal API tokens — account-global, NOT workspace-scoped.
        // Self-authenticating via JWT (manage your own tokens).
        .route(
            "/api/account/tokens",
            get(api_tokens::list_tokens).post(api_tokens::create_token),
        )
        .route(
            "/api/account/tokens/{id}/reveal",
            get(api_tokens::reveal_token),
        )
        .route("/api/account/tokens/{id}", delete(api_tokens::revoke_token))
        // Inbound webhook trigger — PUBLIC, authenticated by a PAT in
        // the Authorization header (not JWT), self-authenticating in the handler.
        .route("/api/webhooks/{trigger_id}", post(webhooks::fire_webhook))
        // Workspace-scoped routes (auth required)
        .nest(
            "/api/workspaces/{workspace_id}",
            Router::new()
                .route("/flows/create", post(flows::create_flow))
                .route("/flows/move", post(flows::move_flow))
                .route("/flows/delete", post(flows::delete_flow))
                .route("/flows/check-expr", post(flows::check_flow_expr))
                .route("/flows/list", get(flows::list_flows))
                .route("/flows/get/{*path}", get(flows::get_flow))
                .route("/flows/run/{*path}", post(flows::run_flow))
                // Schedules (cron)
                .route("/schedules/list", get(schedules::list_schedules))
                .route("/schedules/create", post(schedules::create_schedule))
                .route("/schedules/preview", post(schedules::preview_schedule))
                .route("/schedules/get/{id}", get(schedules::get_schedule))
                .route("/schedules/{id}", put(schedules::update_schedule))
                .route("/schedules/{id}", delete(schedules::delete_schedule))
                .route("/schedules/{id}/enable", post(schedules::enable_schedule))
                .route("/schedules/{id}/run", post(schedules::run_schedule_now))
                // Triggers — webhook + future push types, per flow.
                .route(
                    "/flows/{flow_id}/triggers",
                    get(triggers::list_triggers).post(triggers::create_trigger),
                )
                .route(
                    "/triggers/{id}",
                    put(triggers::update_trigger).delete(triggers::delete_trigger),
                )
                .route("/scripts/create", post(scripts::create_script))
                .route("/scripts/move", post(scripts::move_script))
                .route("/scripts/delete", post(scripts::delete_script))
                .route(
                    "/scripts/references/{script_id}",
                    get(scripts::script_references),
                )
                .route("/scripts/list", get(scripts::list_scripts))
                .route("/scripts/get/hash/{hash}", get(scripts::get_script_by_hash))
                .route(
                    "/scripts/history/{*path}",
                    get(scripts::list_script_versions),
                )
                .route(
                    "/scripts/get/path/{*path}",
                    get(scripts::get_script_by_path),
                )
                // Workspace member info
                .route("/me", get(workspaces::get_me))
                // Run routes
                .route("/runs/create", post(runs::create_run))
                .route("/runs/run_wait_result", post(runs::run_wait_result))
                .route("/runs/list", get(runs::list_runs))
                .route("/runs/get/{run_id}", get(runs::get_run))
                .route("/runs/{run_id}/logs", get(runs::get_run_logs))
                .route("/runs/{run_id}/logs/stream", get(runs::stream_run_logs))
                .route("/runs/{run_id}/cancel", post(runs::cancel_run_handler))
                .route("/runs/{run_id}/rerun", post(runs::rerun_handler))
                .route(
                    "/runs/{run_id}/mark-success",
                    post(runs::mark_success_handler),
                )
                .route("/runs/{run_id}/mark-fail", post(runs::mark_fail_handler))
                // Service log routes
                .route("/services/logs", get(services::get_service_logs))
                .route("/services/logs/stream", get(services::stream_service_logs))
                // Team routes
                .route("/teams/list", get(teams::list_teams))
                .route("/teams/get/{name}", get(teams::get_team))
                .route("/teams/create", post(teams::create_team))
                .route("/teams/delete/{name}", delete(teams::delete_team))
                .route("/teams/{name}/members", get(teams::list_team_members))
                .route("/teams/{name}/members", post(teams::add_team_member))
                .route(
                    "/teams/{name}/members/{email}",
                    delete(teams::remove_team_member),
                )
                .route(
                    "/teams/{name}/members/{email}/role",
                    put(teams::update_team_member_role),
                )
                .route("/teams/{name}/quota", get(teams::get_team_quota))
                .route("/teams/{name}/quota", put(teams::update_team_quota))
                // Member routes
                .route("/members", get(members::list_members))
                .route("/members", post(members::add_member))
                .route("/members/{email}", put(members::update_member_role))
                .route("/members/{email}", delete(members::remove_member))
                // Secrets — workspace-scoped, write-only encrypted store.
                .route("/secrets", get(secrets::list_secrets))
                .route("/secrets", post(secrets::create_secret))
                .route("/secrets/{*path}", put(secrets::rotate_secret))
                .route("/secrets/{*path}", delete(secrets::delete_secret))
                .layer(auth_middleware),
        )
        .with_state(AppState { db, secret_key })
        .layer(middleware::from_fn_with_state(metrics, request_trace))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    // All tests use #[sqlx::test] which creates an isolated test DB per test,
    // runs migrations, and drops it automatically — even on panic.

    fn valid_jwt(email: &str) -> String {
        auth::generate_access_token(email).expect("generate JWT")
    }

    async fn seed_test_member(db: &PgPool, email: &str, ws_id: &str) {
        let hash = "$argon2id$v=19$m=19456,t=2,p=1$AAAAAAAAAAAAAAAAAAAAAA$eFBpFPJyjimZN8tCAsTuRgNxNlFxnWF/5o2sow4q3BA";

        sqlx::query!(
            "INSERT INTO account (email, password_hash) VALUES ($1, $2)",
            email,
            hash
        )
        .execute(db)
        .await
        .unwrap();

        sqlx::query!(
            "INSERT INTO workspace (id, name, owner) VALUES ($1, $2, $3)",
            ws_id,
            "Test Workspace",
            email
        )
        .execute(db)
        .await
        .unwrap();

        sqlx::query!(
            "INSERT INTO workspace_member (workspace_id, email, role) VALUES ($1, $2, 'admin')",
            ws_id,
            email
        )
        .execute(db)
        .await
        .unwrap();
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn health_route_returns_ok_without_auth(pool: PgPool) {
        let response = create_router(
            pool,
            crate::test_helpers::test_metrics(),
            crate::test_helpers::test_secret_key(),
        )
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], br#"{"status":"ok"}"#);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn me_route_returns_401_without_auth(pool: PgPool) {
        let response = create_router(
            pool,
            crate::test_helpers::test_metrics(),
            crate::test_helpers::test_secret_key(),
        )
        .oneshot(
            Request::builder()
                .uri("/api/workspaces/ws-1/me")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn me_route_returns_401_with_invalid_token(pool: PgPool) {
        let response = create_router(
            pool,
            crate::test_helpers::test_metrics(),
            crate::test_helpers::test_secret_key(),
        )
        .oneshot(
            Request::builder()
                .uri("/api/workspaces/ws-1/me")
                .header("Authorization", "Bearer invalid-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn me_route_returns_200_for_member(pool: PgPool) {
        let email = "member@test.local";
        let ws_id = "ws-test";
        seed_test_member(&pool, email, ws_id).await;

        let token = valid_jwt(email);
        let uri = format!("/api/workspaces/{ws_id}/me");

        let response = create_router(
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

        assert_eq!(response.status(), StatusCode::OK);

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["email"], email);
        assert_eq!(json["role"], "admin");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn me_route_returns_403_for_non_member(pool: PgPool) {
        let owner = "owner@test.local";
        let ws_id = "ws-test";
        seed_test_member(&pool, owner, ws_id).await;

        // Create an outsider account that is NOT a member of ws_id
        let outsider = "outsider@test.local";
        let hash = "$argon2id$v=19$m=19456,t=2,p=1$AAAAAAAAAAAAAAAAAAAAAA$eFBpFPJyjimZN8tCAsTuRgNxNlFxnWF/5o2sow4q3BA";
        sqlx::query!(
            "INSERT INTO account (email, password_hash) VALUES ($1, $2)",
            outsider,
            hash
        )
        .execute(&pool)
        .await
        .unwrap();

        let token = valid_jwt(outsider);
        let uri = format!("/api/workspaces/{ws_id}/me");

        let response = create_router(
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

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn test_normalize_path_workspace_scoped() {
        assert_eq!(
            normalize_path("/api/workspaces/default/runs/create"),
            "/api/workspaces/:ws/runs/create"
        );
        assert_eq!(
            normalize_path("/api/workspaces/my-ws/me"),
            "/api/workspaces/:ws/me"
        );
    }

    #[test]
    fn test_normalize_path_runs_with_uuid() {
        assert_eq!(
            normalize_path("/api/workspaces/default/runs/get/550e8400-e29b-41d4-a716-446655440000"),
            "/api/workspaces/:ws/runs/get/:id"
        );
        assert_eq!(
            normalize_path(
                "/api/workspaces/default/runs/550e8400-e29b-41d4-a716-446655440000/logs"
            ),
            "/api/workspaces/:ws/runs/:id/logs"
        );
        assert_eq!(
            normalize_path(
                "/api/workspaces/default/runs/550e8400-e29b-41d4-a716-446655440000/cancel"
            ),
            "/api/workspaces/:ws/runs/:id/cancel"
        );
    }

    #[test]
    fn test_normalize_path_teams() {
        assert_eq!(
            normalize_path("/api/workspaces/default/teams/list"),
            "/api/workspaces/:ws/teams/list"
        );
        assert_eq!(
            normalize_path("/api/workspaces/default/teams/get/engineering"),
            "/api/workspaces/:ws/teams/get/:id"
        );
        assert_eq!(
            normalize_path("/api/workspaces/default/teams/delete/engineering"),
            "/api/workspaces/:ws/teams/delete/:id"
        );
        assert_eq!(
            normalize_path("/api/workspaces/default/teams/engineering/members"),
            "/api/workspaces/:ws/teams/:id/members"
        );
        assert_eq!(
            normalize_path("/api/workspaces/default/teams/engineering/quota"),
            "/api/workspaces/:ws/teams/:id/quota"
        );
    }

    #[test]
    fn test_normalize_path_members() {
        assert_eq!(
            normalize_path("/api/workspaces/default/members"),
            "/api/workspaces/:ws/members"
        );
        assert_eq!(
            normalize_path("/api/workspaces/default/members/alice@example.com"),
            "/api/workspaces/:ws/members/:id"
        );
    }

    #[test]
    fn test_normalize_path_scripts() {
        assert_eq!(
            normalize_path("/api/workspaces/default/scripts/get/hash/abc123def456"),
            "/api/workspaces/:ws/scripts/get/:id/abc123def456"
        );
        assert_eq!(
            normalize_path("/api/workspaces/default/scripts/history/path/to/script.py"),
            "/api/workspaces/:ws/scripts/history/:path"
        );
    }

    #[test]
    fn test_normalize_path_auth_routes() {
        assert_eq!(normalize_path("/api/auth/login"), "/api/auth/login");
        assert_eq!(normalize_path("/api/auth/signup"), "/api/auth/signup");
        assert_eq!(normalize_path("/api/auth/refresh"), "/api/auth/refresh");
    }
}
