//! Cluster dashboard endpoints.
//!
//! These expose cross-workspace worker capacity and the jobs currently running
//! on each worker. Workers are shared infrastructure (the `worker_ping` table is
//! not workspace-scoped), so access is gated on the **instance admin** flag
//! (`account.is_admin`) rather than a workspace role. Routes live at the
//! top level (`/api/admin/cluster/*`) and authenticate themselves, mirroring
//! `users::search_users`.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::ApiError;

/// A worker is considered alive if it has pinged within this window. The worker
/// heartbeat runs every 30s, so this tolerates two missed pings.
///
/// Public because the liveness reaper's threshold lower-bound is anchored to it
/// (`COVEFLOW_WORKER_REAP_AFTER_SECS` must be >= this; see `main.rs`). Keeping a
/// single definition avoids the two values drifting if the heartbeat interval
/// changes.
pub const STALE_AFTER_SECONDS: i64 = 90;

#[derive(serde::Serialize)]
pub struct ClusterSummary {
    pub total_cpus: f32,
    pub used_cpus: f32,
    /// used / total, in [0, 1]; 0 when no capacity is registered.
    pub utilization: f32,
    pub workers_total: i64,
    pub workers_alive: i64,
}

#[derive(serde::Serialize)]
pub struct UsageF32 {
    pub used: f32,
    pub total: f32,
}

#[derive(serde::Serialize)]
pub struct UsageI64 {
    pub used: i64,
    pub total: i64,
}

#[derive(serde::Serialize)]
pub struct ClusterWorker {
    /// Unique per-process identity (worker_ping PK) — used as the URL key.
    pub worker: String,
    /// Operator-friendly name shown in the UI; several live processes may share
    /// it. Falls back to `worker` for pre-migration rows.
    pub display_name: String,
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox_mode: Option<String>,
    /// "alive" or "stale".
    pub status: &'static str,
    pub ping_at: DateTime<Utc>,
    pub cpus: UsageF32,
    pub memory_mb: UsageI64,
    pub disk_mb: UsageI64,
    pub cpu_usage_percent: f32,
    pub running_jobs: i64,
}

/// Bounded list of a worker's running jobs. `has_more` flags truncation so the
/// UI can say "showing N of more" instead of implying it is the full set.
#[derive(serde::Serialize)]
pub struct ClusterWorkerRunsResponse {
    pub items: Vec<ClusterWorkerRun>,
    pub has_more: bool,
}

/// Max running-job rows returned per worker (see ClusterWorkerRunsResponse).
const WORKER_RUNS_LIMIT: usize = 200;
/// Max accepted worker-name length == worker_ping.worker VARCHAR(100). A longer
/// name can never match a row, so reject it as a clear 400 rather than returning
/// a confusing empty "no running jobs".
const MAX_WORKER_NAME_LEN: usize = 100;

#[derive(serde::Serialize)]
pub struct ClusterWorkerRun {
    pub run_id: Uuid,
    pub workspace_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub script_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    pub cpus: f32,
    pub memory_mb: i32,
    pub disk_mb: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    pub tag: String,
}

/// Authenticate the caller and require the instance-admin flag. Records the
/// caller into the current span's `admin_email` field *before* the authz check,
/// so denied escalation attempts (403) are attributable in traces too.
async fn require_instance_admin(db: &PgPool, headers: &HeaderMap) -> Result<String, ApiError> {
    let email = crate::auth::email_from_bearer_headers(headers)?;
    tracing::Span::current().record("admin_email", email.as_str());

    let is_admin = sqlx::query_scalar!(
        r#"SELECT is_admin AS "is_admin!" FROM account WHERE email = $1"#,
        email,
    )
    .fetch_optional(db)
    .await?
    .unwrap_or(false);

    if !is_admin {
        return Err(ApiError::Forbidden("instance admin required".into()));
    }
    Ok(email)
}

#[tracing::instrument(
    name = "api::cluster_summary",
    skip(db, headers),
    fields(admin_email = tracing::field::Empty)
)]
pub async fn cluster_summary(
    State(db): State<PgPool>,
    headers: HeaderMap,
) -> Result<Json<ClusterSummary>, ApiError> {
    require_instance_admin(&db, &headers).await?;

    // Total capacity counts only alive workers: a worker that stopped pinging may
    // already be dead (its row lingers until the reaper sweeps it), so summing its
    // advertised capacity would inflate utilization with phantom resources.
    // workers_total still counts every row so operators can see the registered vs
    // alive gap. `used` is derived from running jobs on alive workers (consistent
    // with the per-worker view), not the lagging worker_ping.used_* snapshot.
    let row = sqlx::query!(
        r#"
        SELECT
            COALESCE(SUM(w.total_cpus) FILTER (
                WHERE w.ping_at > now() - make_interval(secs => $1)
            ), 0)::real                        AS "total_cpus!",
            COALESCE((
                SELECT SUM(r.cpus)
                FROM run_queue q
                JOIN run r ON r.id = q.id
                JOIN worker_ping w2 ON w2.worker = q.worker
                WHERE q.running = TRUE
                  AND w2.ping_at > now() - make_interval(secs => $1)
            ), 0)::real                        AS "used_cpus!",
            COUNT(*)                           AS "workers_total!",
            COUNT(*) FILTER (
                WHERE w.ping_at > now() - make_interval(secs => $1)
            )                                  AS "workers_alive!"
        FROM worker_ping w
        "#,
        STALE_AFTER_SECONDS as f64,
    )
    .fetch_one(&db)
    .await?;

    // Clamp: a worker restart can shrink total below an already-used figure,
    // which would otherwise render as e.g. "130% utilization".
    let utilization = if row.total_cpus > 0.0 {
        (row.used_cpus / row.total_cpus).clamp(0.0, 1.0)
    } else {
        0.0
    };

    Ok(Json(ClusterSummary {
        total_cpus: row.total_cpus,
        used_cpus: row.used_cpus,
        utilization,
        workers_total: row.workers_total,
        workers_alive: row.workers_alive,
    }))
}

#[tracing::instrument(
    name = "api::cluster_workers",
    skip(db, headers),
    fields(admin_email = tracing::field::Empty)
)]
pub async fn cluster_workers(
    State(db): State<PgPool>,
    headers: HeaderMap,
) -> Result<Json<Vec<ClusterWorker>>, ApiError> {
    require_instance_admin(&db, &headers).await?;

    // `used` is derived live from the worker's currently-running jobs (the same
    // run_queue source as running_jobs), not from worker_ping.used_*. The ping
    // columns are only a ~30s snapshot written by periodic_ping, so they lag the
    // live job count and never capture sub-interval jobs — pairing a stale gauge
    // with a live count made the bars read 0 while Jobs showed >0. Summing the
    // jobs' requested resources keeps the bars consistent with Jobs and free of
    // the f32 drift the in-memory accumulator accrues. (Capacity totals stay on
    // worker_ping; dedicated reservations, if added, won't appear here.)
    let rows = sqlx::query!(
        r#"
        SELECT
            w.worker                               AS "worker!",
            COALESCE(w.display_name, w.worker)     AS "display_name!",
            w.tags                                 AS "tags!",
            w.sandbox_mode                         AS "sandbox_mode",
            w.ping_at                              AS "ping_at!",
            COALESCE(w.total_cpus, 0)::real        AS "total_cpus!",
            COALESCE(j.used_cpus, 0)::real         AS "used_cpus!",
            COALESCE(w.total_memory_mb, 0)         AS "total_memory_mb!",
            COALESCE(j.used_memory_mb, 0)          AS "used_memory_mb!",
            COALESCE(w.total_disk_mb, 0)           AS "total_disk_mb!",
            COALESCE(j.used_disk_mb, 0)            AS "used_disk_mb!",
            COALESCE(w.cpu_usage_percent, 0)::real AS "cpu_usage_percent!",
            (w.ping_at > now() - make_interval(secs => $1)) AS "alive!",
            COALESCE(j.running_jobs, 0)            AS "running_jobs!"
        FROM worker_ping w
        LEFT JOIN (
            SELECT q.worker                      AS worker,
                   COUNT(*)                      AS running_jobs,
                   COALESCE(SUM(r.cpus), 0)::real AS used_cpus,
                   COALESCE(SUM(r.memory_mb), 0) AS used_memory_mb,
                   COALESCE(SUM(r.disk_mb), 0)   AS used_disk_mb
            FROM run_queue q
            JOIN run r ON r.id = q.id
            WHERE q.running = TRUE AND q.worker IS NOT NULL
            GROUP BY q.worker
        ) j ON j.worker = w.worker
        ORDER BY w.worker
        "#,
        STALE_AFTER_SECONDS as f64,
    )
    .fetch_all(&db)
    .await?;

    let workers = rows
        .into_iter()
        .map(|r| ClusterWorker {
            worker: r.worker,
            display_name: r.display_name,
            tags: r.tags,
            sandbox_mode: r.sandbox_mode,
            status: if r.alive { "alive" } else { "stale" },
            ping_at: r.ping_at,
            cpus: UsageF32 {
                used: r.used_cpus,
                total: r.total_cpus,
            },
            memory_mb: UsageI64 {
                used: r.used_memory_mb,
                total: r.total_memory_mb,
            },
            disk_mb: UsageI64 {
                used: r.used_disk_mb,
                total: r.total_disk_mb,
            },
            cpu_usage_percent: r.cpu_usage_percent,
            running_jobs: r.running_jobs,
        })
        .collect();

    Ok(Json(workers))
}

#[tracing::instrument(
    name = "api::cluster_worker_runs",
    skip(db, headers),
    fields(admin_email = tracing::field::Empty)
)]
pub async fn cluster_worker_runs(
    State(db): State<PgPool>,
    headers: HeaderMap,
    Path(worker): Path<String>,
) -> Result<Json<ClusterWorkerRunsResponse>, ApiError> {
    require_instance_admin(&db, &headers).await?;

    if worker.len() > MAX_WORKER_NAME_LEN {
        return Err(ApiError::BadRequest("worker name too long".into()));
    }

    // Bounded: a stale worker can accumulate many running=TRUE rows (jobs killed
    // without unclaim). Fetch one past the limit to detect truncation. Most
    // recent first.
    let rows = sqlx::query!(
        r#"
        SELECT
            r.id           AS "run_id!",
            r.workspace_id AS "workspace_id!",
            r.script_path  AS "script_path",
            r.language     AS "language",
            r.cpus         AS "cpus!",
            r.memory_mb    AS "memory_mb!",
            r.disk_mb      AS "disk_mb!",
            q.started_at   AS "started_at",
            q.tag          AS "tag!"
        FROM run_queue q
        JOIN run r ON r.id = q.id
        WHERE q.worker = $1 AND q.running = TRUE
        ORDER BY q.started_at DESC NULLS LAST
        LIMIT $2
        "#,
        worker,
        WORKER_RUNS_LIMIT as i64 + 1,
    )
    .fetch_all(&db)
    .await?;

    let has_more = rows.len() > WORKER_RUNS_LIMIT;
    let items = rows
        .into_iter()
        .take(WORKER_RUNS_LIMIT)
        .map(|r| ClusterWorkerRun {
            run_id: r.run_id,
            workspace_id: r.workspace_id,
            script_path: r.script_path,
            language: r.language,
            cpus: r.cpus,
            memory_mb: r.memory_mb,
            disk_mb: r.disk_mb,
            started_at: r.started_at,
            tag: r.tag,
        })
        .collect();

    Ok(Json(ClusterWorkerRunsResponse { items, has_more }))
}

#[cfg(test)]
mod tests {
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use sqlx::PgPool;
    use tower::ServiceExt;

    use crate::test_helpers::*;

    /// `used_cpus` here is the worker_ping snapshot column, which the dashboard
    /// no longer reads for capacity — `used` is derived from running jobs. Tests
    /// pass deliberately-wrong values to prove the snapshot is ignored.
    async fn seed_worker_ping(pool: &PgPool, worker: &str, total_cpus: f32, used_cpus: f32) {
        sqlx::query!(
            r#"INSERT INTO worker_ping
                (worker, tags, sandbox_mode, total_cpus, used_cpus,
                 total_memory_mb, used_memory_mb, total_disk_mb, used_disk_mb,
                 cpu_usage_percent)
               VALUES ($1, ARRAY['default'], 'nsjail', $2, $3, 32768, 4096, 102400, 8192, 12.5)"#,
            worker,
            total_cpus,
            used_cpus,
        )
        .execute(pool)
        .await
        .unwrap();
    }

    /// Insert a running job (run + run_queue) on a worker, with the given
    /// requested resources, so the cluster queries see live usage.
    async fn seed_running_job(
        pool: &PgPool,
        ws: &str,
        worker: &str,
        cpus: f32,
        memory_mb: i32,
        disk_mb: i32,
    ) {
        sqlx::query!(
            "INSERT INTO workspace (id, name, owner) VALUES ($1, 'Test', 'o@test.local')
             ON CONFLICT (id) DO NOTHING",
            ws,
        )
        .execute(pool)
        .await
        .unwrap();
        let run_id = sqlx::query_scalar!(
            "INSERT INTO run (workspace_id, kind, cpus, memory_mb, disk_mb, created_by)
             VALUES ($1, 'script', $2, $3, $4, 'o@test.local') RETURNING id",
            ws,
            cpus,
            memory_mb,
            disk_mb,
        )
        .fetch_one(pool)
        .await
        .unwrap();
        sqlx::query!(
            "INSERT INTO run_queue (id, running, worker, started_at)
             VALUES ($1, TRUE, $2, now())",
            run_id,
            worker,
        )
        .execute(pool)
        .await
        .unwrap();
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn summary_aggregates_for_instance_admin(pool: PgPool) {
        let admin = "admin@test.local";
        seed_instance_admin(&pool, admin).await;
        seed_worker_ping(&pool, "worker-1", 8.0, 4.0).await;
        seed_worker_ping(&pool, "worker-2", 4.0, 1.0).await;

        let token = valid_jwt(admin);
        let response =
            crate::create_router(pool, test_metrics(), crate::test_helpers::test_secret_key())
                .oneshot(
                    Request::builder()
                        .uri("/api/admin/cluster/summary")
                        .header("Authorization", format!("Bearer {token}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["total_cpus"], 12.0);
        // No running jobs → used is 0 regardless of the worker_ping snapshot.
        assert_eq!(json["used_cpus"], 0.0);
        assert_eq!(json["workers_total"], 2);
        assert_eq!(json["workers_alive"], 2);
        assert_eq!(json["utilization"].as_f64().unwrap(), 0.0);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn summary_used_comes_from_running_jobs(pool: PgPool) {
        let admin = "admin@test.local";
        seed_instance_admin(&pool, admin).await;
        // Snapshot says 9 cpus used — a lie the dashboard must ignore.
        seed_worker_ping(&pool, "worker-1", 8.0, 9.0).await;
        seed_running_job(&pool, "ws-1", "worker-1", 2.0, 1024, 2048).await;
        seed_running_job(&pool, "ws-1", "worker-1", 1.0, 512, 1024).await;

        let token = valid_jwt(admin);
        let response =
            crate::create_router(pool, test_metrics(), crate::test_helpers::test_secret_key())
                .oneshot(
                    Request::builder()
                        .uri("/api/admin/cluster/summary")
                        .header("Authorization", format!("Bearer {token}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["total_cpus"], 8.0);
        // 2.0 + 1.0 from the running jobs, not the bogus 9.0 snapshot.
        assert_eq!(json["used_cpus"], 3.0);
        let util = json["utilization"].as_f64().unwrap();
        assert!((util - 3.0 / 8.0).abs() < 1e-4);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn summary_excludes_stale_workers_from_capacity(pool: PgPool) {
        let admin = "admin@test.local";
        seed_instance_admin(&pool, admin).await;
        seed_worker_ping(&pool, "worker-1", 8.0, 4.0).await;
        seed_worker_ping(&pool, "worker-2", 4.0, 1.0).await;
        // Backdate worker-2 well past the stale window: its capacity must not
        // count toward total/used, but it still counts toward workers_total.
        sqlx::query!(
            "UPDATE worker_ping SET ping_at = now() - interval '10 minutes' WHERE worker = 'worker-2'"
        )
        .execute(&pool)
        .await
        .unwrap();

        let token = valid_jwt(admin);
        let response =
            crate::create_router(pool, test_metrics(), crate::test_helpers::test_secret_key())
                .oneshot(
                    Request::builder()
                        .uri("/api/admin/cluster/summary")
                        .header("Authorization", format!("Bearer {token}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        // Only worker-1's capacity counts; no running jobs → used 0.
        assert_eq!(json["total_cpus"], 8.0);
        assert_eq!(json["used_cpus"], 0.0);
        assert_eq!(json["workers_total"], 2);
        assert_eq!(json["workers_alive"], 1);
        assert_eq!(json["utilization"].as_f64().unwrap(), 0.0);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn workers_lists_capacity_and_running_jobs(pool: PgPool) {
        let admin = "admin@test.local";
        seed_instance_admin(&pool, admin).await;
        seed_worker_ping(&pool, "worker-1", 8.0, 2.0).await;

        let token = valid_jwt(admin);
        let response =
            crate::create_router(pool, test_metrics(), crate::test_helpers::test_secret_key())
                .oneshot(
                    Request::builder()
                        .uri("/api/admin/cluster/workers")
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
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["worker"], "worker-1");
        assert_eq!(items[0]["status"], "alive");
        assert_eq!(items[0]["cpus"]["total"], 8.0);
        assert_eq!(items[0]["running_jobs"], 0);
        // No jobs → used is 0 even though the snapshot seeded used_cpus = 2.0.
        assert_eq!(items[0]["cpus"]["used"], 0.0);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn worker_used_reflects_running_jobs_not_snapshot(pool: PgPool) {
        let admin = "admin@test.local";
        seed_instance_admin(&pool, admin).await;
        // Snapshot lies (used_cpus = 7.0); the bars must come from the jobs.
        seed_worker_ping(&pool, "worker-1", 8.0, 7.0).await;
        seed_running_job(&pool, "ws-1", "worker-1", 2.0, 1024, 2048).await;
        seed_running_job(&pool, "ws-1", "worker-1", 1.5, 512, 1024).await;

        let token = valid_jwt(admin);
        let response =
            crate::create_router(pool, test_metrics(), crate::test_helpers::test_secret_key())
                .oneshot(
                    Request::builder()
                        .uri("/api/admin/cluster/workers")
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
        assert_eq!(items[0]["running_jobs"], 2);
        assert_eq!(items[0]["cpus"]["used"], 3.5); // 2.0 + 1.5
        assert_eq!(items[0]["cpus"]["total"], 8.0);
        assert_eq!(items[0]["memory_mb"]["used"], 1536); // 1024 + 512
        assert_eq!(items[0]["disk_mb"]["used"], 3072); // 2048 + 1024
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn non_instance_admin_is_forbidden(pool: PgPool) {
        let user = "editor@test.local";
        seed_account(&pool, user).await;
        seed_workspace_member(&pool, "ws-1", user, "admin").await; // workspace admin, not instance

        let token = valid_jwt(user);
        let response =
            crate::create_router(pool, test_metrics(), crate::test_helpers::test_secret_key())
                .oneshot(
                    Request::builder()
                        .uri("/api/admin/cluster/summary")
                        .header("Authorization", format!("Bearer {token}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn unauthenticated_is_rejected(pool: PgPool) {
        let response =
            crate::create_router(pool, test_metrics(), crate::test_helpers::test_secret_key())
                .oneshot(
                    Request::builder()
                        .uri("/api/admin/cluster/workers")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn worker_runs_returns_bounded_envelope(pool: PgPool) {
        let admin = "admin@test.local";
        seed_instance_admin(&pool, admin).await;

        let token = valid_jwt(admin);
        let response =
            crate::create_router(pool, test_metrics(), crate::test_helpers::test_secret_key())
                .oneshot(
                    Request::builder()
                        .uri("/api/admin/cluster/workers/worker-x/runs")
                        .header("Authorization", format!("Bearer {token}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["items"].as_array().unwrap().is_empty());
        assert_eq!(json["has_more"], false);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn worker_runs_rejects_overlong_name(pool: PgPool) {
        let admin = "admin@test.local";
        seed_instance_admin(&pool, admin).await;

        let token = valid_jwt(admin);
        let long = "w".repeat(200);
        let response =
            crate::create_router(pool, test_metrics(), crate::test_helpers::test_secret_key())
                .oneshot(
                    Request::builder()
                        .uri(format!("/api/admin/cluster/workers/{long}/runs"))
                        .header("Authorization", format!("Bearer {token}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
