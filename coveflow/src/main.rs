use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use coveflow_worker::{
    CoveflowMetrics, NsjailConfig, SandboxMode, WorkerConfig, init_db_log_layer, run_worker,
};
use prometheus_client::registry::Registry;
use sqlx::postgres::PgPoolOptions;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

const DEFAULT_DATABASE_URL: &str = "postgres://postgres:changeme@localhost/coveflow";
const DEFAULT_API_ADDR: &str = "127.0.0.1:8000";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RunMode {
    Api,
    Worker,
    All,
}

impl RunMode {
    fn service_name(self) -> &'static str {
        match self {
            Self::Api => "api",
            Self::Worker => "worker",
            Self::All => "api-worker",
        }
    }
}

impl FromStr for RunMode {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "api" | "server" => Ok(Self::Api),
            "worker" => Ok(Self::Worker),
            "all" | "both" => Ok(Self::All),
            other => Err(anyhow!(
                "invalid mode '{other}', expected one of: api, worker, all"
            )),
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();

    let mode = run_mode()?;
    let database_url =
        env_first(&["DATABASE_URL"]).unwrap_or_else(|| DEFAULT_DATABASE_URL.to_string());
    let default_db_max_connections: u32 = match mode {
        RunMode::Api | RunMode::Worker => 10,
        RunMode::All => 20,
    };
    let db_max_connections = env_parse("COVEFLOW_DB_MAX_CONNECTIONS", default_db_max_connections)?;
    let run_migrations = env_parse_bool("COVEFLOW_RUN_MIGRATIONS", true)?;

    let db = PgPoolOptions::new()
        .max_connections(db_max_connections)
        .connect(&database_url)
        .await
        .context("failed to connect to database")?;

    let otel_provider = init_tracing(&db, mode)?;

    tracing::info!(
        mode = ?mode,
        db_max_connections,
        run_migrations,
        "coveflow starting"
    );

    if run_migrations {
        tracing::info!("running database migrations");
        sqlx::migrate!("./migrations")
            .run(&db)
            .await
            .context("failed to run database migrations")?;
    }

    // Fail-fast: production safety guards. Must run BEFORE bootstrap so a
    // misconfigured deployment exits before writing any admin account (otherwise
    // a later restart with a fixed JWT_SECRET hits the ON CONFLICT path and the
    // first password is silently kept).
    if matches!(mode, RunMode::Api | RunMode::All) {
        validate_jwt_secret()?;
    }
    // The secret-store key is required in every mode (API encrypts, worker
    // decrypts). Parse once and pass the validated key down (no re-parsing).
    let secret_key = validate_secret_key()?;

    // Optional env-driven instance-admin bootstrap. Lets a fresh deployment come
    // up with a usable admin without manual SQL. Only relevant where the API runs.
    if matches!(mode, RunMode::Api | RunMode::All) {
        bootstrap_admin_from_env(&db).await?;
    }

    // Metrics registry — shared by API, worker, and metrics server
    let mut registry = Registry::default();
    let worker_metrics = Arc::new(CoveflowMetrics::new(&mut registry));
    let api_metrics = Arc::new(coveflow_api::ApiMetrics::register(&mut registry));
    let registry = Arc::new(registry);

    let cancel = CancellationToken::new();

    // Metrics HTTP server is opt-in: only starts when COVEFLOW_METRICS_ADDR is set
    let metrics_server = if let Some(metrics_addr) = metrics_addr_from_env()? {
        Some(tokio::spawn(run_metrics_server(
            registry,
            metrics_addr,
            cancel.clone(),
        )))
    } else {
        tracing::info!("COVEFLOW_METRICS_ADDR not set, metrics server disabled");
        None
    };

    match mode {
        RunMode::Api => {
            let api_addr = api_addr_from_env()?;
            run_api_only(db, api_addr, api_metrics, secret_key, cancel.clone()).await?;
        }
        RunMode::Worker => {
            let mut worker_config = worker_config_from_env()?;
            worker_config.secret_key = Some(secret_key);
            run_worker_only(db, worker_config, worker_metrics, cancel.clone()).await?;
        }
        RunMode::All => {
            let api_addr = api_addr_from_env()?;
            let mut worker_config = worker_config_from_env()?;
            worker_config.secret_key = Some(secret_key.clone());
            run_api_and_worker(
                db,
                api_addr,
                worker_config,
                api_metrics,
                worker_metrics,
                secret_key,
                cancel.clone(),
            )
            .await?;
        }
    }

    cancel.cancel();
    if let Some(handle) = metrics_server {
        let _ = handle.await;
    }

    if let Some(ref provider) = otel_provider {
        if let Err(e) = provider.shutdown() {
            tracing::warn!(error = %e, "OpenTelemetry provider shutdown failed, some traces may be lost");
        }
    }

    tracing::info!("coveflow stopped");
    Ok(())
}

fn init_tracing(
    db: &sqlx::PgPool,
    mode: RunMode,
) -> Result<Option<opentelemetry_sdk::trace::SdkTracerProvider>> {
    // RUST_LOG overrides this default filter, for example: RUST_LOG=info,coveflow_worker=debug.
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new(
            "info,coveflow=info,coveflow_api=info,coveflow_worker=info,coveflow_queue=info",
        )
    });

    let service =
        env_first(&["COVEFLOW_SERVICE_NAME"]).unwrap_or_else(|| mode.service_name().to_string());
    let instance_id =
        env_first(&["COVEFLOW_INSTANCE_ID"]).unwrap_or_else(|| default_instance_id(&service));

    let db_layer = init_db_log_layer(db.clone(), instance_id.clone(), service.clone());

    // Optional: OpenTelemetry → Tempo (via OTLP)
    // OTEL_EXPORTER_OTLP_ENDPOINT env var is read by the OTLP exporter itself.
    let mut otel_provider: Option<opentelemetry_sdk::trace::SdkTracerProvider> = None;
    let otel_layer = if env_first(&["OTEL_EXPORTER_OTLP_ENDPOINT"]).is_some() {
        use opentelemetry::trace::TracerProvider;

        let exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_tonic()
            .build()
            .map_err(|e| anyhow!("failed to build OTLP span exporter: {e}"))?;

        let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
            .with_batch_exporter(exporter)
            .with_resource(
                opentelemetry_sdk::Resource::builder()
                    .with_service_name(service.clone())
                    .build(),
            )
            .build();

        let tracer = provider.tracer("coveflow");
        otel_provider = Some(provider);

        Some(tracing_opentelemetry::layer().with_tracer(tracer))
    } else {
        None
    };

    // Optional: Loki log shipping
    let loki_layer = if let Some(loki_endpoint) = env_first(&["LOKI_ENDPOINT"]) {
        let loki_url: url::Url = loki_endpoint
            .parse()
            .with_context(|| format!("invalid LOKI_ENDPOINT '{loki_endpoint}'"))?;

        let (layer, task) = tracing_loki::builder()
            .label("service", service.clone())
            .context("failed to set loki service label")?
            .build_url(loki_url)
            .context("failed to build loki layer")?;

        tokio::spawn(async move {
            task.await;
            tracing::warn!("loki background task exited, log shipping may have stopped");
        });
        Some(layer)
    } else {
        None
    };

    tracing_subscriber::registry()
        .with(env_filter)
        .with(tracing_subscriber::fmt::layer())
        .with(db_layer)
        .with(otel_layer)
        .with(loki_layer)
        .try_init()
        .context("failed to initialize tracing subscriber")?;

    Ok(otel_provider)
}

async fn run_metrics_server(
    registry: Arc<Registry>,
    addr: SocketAddr,
    cancel: CancellationToken,
) -> Result<()> {
    use axum::extract::State;
    use axum::http::header;
    use axum::response::IntoResponse;
    use axum::routing::get;

    async fn handler(State(registry): State<Arc<Registry>>) -> impl IntoResponse {
        let mut buf = String::new();
        prometheus_client::encoding::text::encode(&mut buf, &registry)
            .expect("prometheus encoding should not fail");
        (
            [(
                header::CONTENT_TYPE,
                "application/openmetrics-text; version=1.0.0; charset=utf-8",
            )],
            buf,
        )
    }

    let app = axum::Router::new()
        .route("/metrics", get(handler))
        .with_state(registry);

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind metrics listener on {addr}"))?;

    tracing::info!(%addr, "metrics server listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            cancel.cancelled().await;
        })
        .await
        .context("metrics server failed")?;

    Ok(())
}

async fn run_api_only(
    db: sqlx::PgPool,
    addr: SocketAddr,
    metrics: Arc<coveflow_api::ApiMetrics>,
    secret_key: coveflow_types::crypto::SecretKey,
    cancel: CancellationToken,
) -> Result<()> {
    let api = tokio::spawn(run_api_server(
        db,
        addr,
        metrics,
        secret_key,
        cancel.clone(),
    ));
    supervise_one("api", api, cancel).await
}

async fn run_worker_only(
    db: sqlx::PgPool,
    config: WorkerConfig,
    metrics: Arc<CoveflowMetrics>,
    cancel: CancellationToken,
) -> Result<()> {
    let worker = tokio::spawn(run_worker_task(db, config, metrics, cancel.clone()));
    supervise_one("worker", worker, cancel).await
}

async fn run_api_and_worker(
    db: sqlx::PgPool,
    addr: SocketAddr,
    config: WorkerConfig,
    api_metrics: Arc<coveflow_api::ApiMetrics>,
    worker_metrics: Arc<CoveflowMetrics>,
    secret_key: coveflow_types::crypto::SecretKey,
    cancel: CancellationToken,
) -> Result<()> {
    let api = tokio::spawn(run_api_server(
        db.clone(),
        addr,
        api_metrics,
        secret_key,
        cancel.clone(),
    ));
    let worker = tokio::spawn(run_worker_task(db, config, worker_metrics, cancel.clone()));
    supervise_two(api, worker, cancel).await
}

async fn run_api_server(
    db: sqlx::PgPool,
    addr: SocketAddr,
    metrics: Arc<coveflow_api::ApiMetrics>,
    secret_key: coveflow_types::crypto::SecretKey,
    cancel: CancellationToken,
) -> Result<()> {
    // Background liveness reaper: fail jobs of dead workers and prune stale
    // worker_ping rows. Lives in the API process so a worker-only deployment
    // doesn't duplicate it.
    spawn_reaper(db.clone(), reap_after_secs_from_env()?, cancel.clone());
    spawn_scheduler(db.clone(), cancel.clone());

    let app = coveflow_api::create_router(db, metrics, secret_key);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind API listener on {addr}"))?;

    tracing::info!(%addr, "api server listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            cancel.cancelled().await;
            tracing::info!("api server shutdown requested");
        })
        .await
        .context("api server failed")?;

    tracing::info!("api server stopped");
    Ok(())
}

async fn run_worker_task(
    db: sqlx::PgPool,
    config: WorkerConfig,
    metrics: Arc<CoveflowMetrics>,
    cancel: CancellationToken,
) -> Result<()> {
    run_worker(db, config, metrics, cancel).await;
    Ok(())
}

async fn supervise_one(
    name: &'static str,
    handle: JoinHandle<Result<()>>,
    cancel: CancellationToken,
) -> Result<()> {
    let mut handle = handle;
    tokio::select! {
        result = &mut handle => task_result(name, result),
        () = shutdown_signal() => {
            tracing::info!("shutdown signal received");
            cancel.cancel();
            await_task(name, handle).await
        }
    }
}

async fn supervise_two(
    api: JoinHandle<Result<()>>,
    worker: JoinHandle<Result<()>>,
    cancel: CancellationToken,
) -> Result<()> {
    let mut api = api;
    let mut worker = worker;
    tokio::select! {
        result = &mut api => {
            cancel.cancel();
            let api_result = task_result("api", result);
            let worker_result = await_task("worker", worker).await;
            api_result.and(worker_result)
        }
        result = &mut worker => {
            cancel.cancel();
            let worker_result = task_result("worker", result);
            let api_result = await_task("api", api).await;
            worker_result.and(api_result)
        }
        () = shutdown_signal() => {
            tracing::info!("shutdown signal received");
            cancel.cancel();
            let api_result = await_task("api", api).await;
            let worker_result = await_task("worker", worker).await;
            api_result.and(worker_result)
        }
    }
}

async fn await_task(name: &'static str, handle: JoinHandle<Result<()>>) -> Result<()> {
    task_result(name, handle.await)
}

fn task_result(
    name: &'static str,
    result: std::result::Result<Result<()>, tokio::task::JoinError>,
) -> Result<()> {
    match result {
        Ok(inner) => inner,
        Err(e) => Err(anyhow!("{name} task join failed: {e}")),
    }
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .map_err(|e| tracing::warn!(error = %e, "failed to install SIGTERM handler"))
            .ok();

        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                if let Err(e) = result {
                    tracing::warn!(error = %e, "failed to wait for Ctrl-C");
                }
            }
            () = async {
                match sigterm.as_mut() {
                    Some(signal) => {
                        signal.recv().await;
                    }
                    None => std::future::pending::<()>().await,
                }
            } => {}
        }
    }

    #[cfg(not(unix))]
    {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::warn!(error = %e, "failed to wait for Ctrl-C");
        }
    }
}

fn run_mode() -> Result<RunMode> {
    let arg_mode = mode_arg_from_cli()?;
    let raw_mode = arg_mode
        .or_else(|| env_first(&["COVEFLOW_MODE"]))
        .unwrap_or_else(|| "all".to_string());
    raw_mode.parse()
}

fn mode_arg_from_cli() -> Result<Option<String>> {
    let mut args = std::env::args().skip(1);
    let Some(arg) = args.next() else {
        return Ok(None);
    };

    if arg == "--mode" {
        return args
            .next()
            .map(Some)
            .ok_or_else(|| anyhow!("--mode requires a value"));
    }
    if let Some(value) = arg.strip_prefix("--mode=") {
        return Ok(Some(value.to_string()));
    }
    if matches!(arg.as_str(), "api" | "server" | "worker" | "all" | "both") {
        return Ok(Some(arg));
    }

    Err(anyhow!("unknown argument '{arg}'"))
}

fn validate_jwt_secret() -> Result<()> {
    match std::env::var("JWT_SECRET") {
        Ok(value) if !value.trim().is_empty() => Ok(()),
        _ => Err(anyhow!(
            "JWT_SECRET must be set when running in api/all mode — \
             a missing secret would let anyone forge JWTs and impersonate any account"
        )),
    }
}

/// Parse the Secret-store master key from `COVEFLOW_SECRET_KEY` (base64 32 bytes).
/// Required in every mode (the API encrypts on write, the worker decrypts on
/// inject); fail-fast so a missing/wrong key never lets a deployment silently run
/// without working secrets. Mirrors `validate_jwt_secret`.
fn validate_secret_key() -> Result<coveflow_types::crypto::SecretKey> {
    let raw = std::env::var("COVEFLOW_SECRET_KEY").map_err(|_| {
        anyhow!(
            "COVEFLOW_SECRET_KEY must be set — it encrypts the secret store. \
             Generate one with `openssl rand -base64 32`"
        )
    })?;
    coveflow_types::crypto::SecretKey::from_base64(&raw)
        .map_err(|e| anyhow!("COVEFLOW_SECRET_KEY is invalid: {e}"))
}

fn default_instance_id(service: &str) -> String {
    hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| format!("{service}-{}", std::process::id()))
}

fn metrics_addr_from_env() -> Result<Option<SocketAddr>> {
    match env_first(&["COVEFLOW_METRICS_ADDR"]) {
        Some(raw) => {
            let addr = raw
                .parse::<SocketAddr>()
                .with_context(|| format!("invalid metrics bind address '{raw}'"))?;
            Ok(Some(addr))
        }
        None => Ok(None),
    }
}

fn api_addr_from_env() -> Result<SocketAddr> {
    let raw = env_first(&["COVEFLOW_API_ADDR", "API_ADDR", "BIND_ADDR"])
        .unwrap_or_else(|| DEFAULT_API_ADDR.to_string());
    raw.parse::<SocketAddr>()
        .with_context(|| format!("invalid API bind address '{raw}'"))
}

/// Default liveness-reaper threshold: a worker idle this long (no heartbeat) has
/// its running jobs failed and its `worker_ping` row removed. Ten missed pings
/// at the 30s heartbeat interval — long enough to ride out a transient blip,
/// short enough to free concurrency budget promptly.
const DEFAULT_REAP_AFTER_SECS: i64 = 300;

fn reap_after_secs_from_env() -> Result<i64> {
    let secs: i64 = env_parse("COVEFLOW_WORKER_REAP_AFTER_SECS", DEFAULT_REAP_AFTER_SECS)?;
    // Must stay above the dashboard's stale window: reaping inside it would race
    // the UI badge and risk failing a worker that is merely a ping behind. Anchor
    // to the dashboard constant so the two cannot drift. fail-fast (do not clamp)
    // so a misconfig surfaces at startup instead of silently changing behavior.
    let min = coveflow_api::cluster::STALE_AFTER_SECONDS;
    if secs < min {
        return Err(anyhow!(
            "COVEFLOW_WORKER_REAP_AFTER_SECS must be >= {min} (the dashboard worker stale window); got {secs}"
        ));
    }
    Ok(secs)
}

/// Spawn the background loop that reaps lost workers. Runs in the API process
/// (the single coordinator); the sweep is idempotent so multiple replicas are
/// safe. Bound to `cancel` for graceful shutdown.
/// Background cron scheduler: every 5s, fire any due schedules. Lives in the API
/// process (like the reaper) so a worker-only deployment doesn't duplicate it;
/// multiple API instances are safe via `FOR UPDATE SKIP LOCKED`.
fn spawn_scheduler(db: sqlx::PgPool, cancel: CancellationToken) {
    let tick = Duration::from_secs(5);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tick);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        tracing::info!(tick_secs = tick.as_secs(), "cron scheduler started");
        loop {
            tokio::select! {
                () = cancel.cancelled() => break,
                _ = interval.tick() => {
                    match coveflow_queue::run_due_schedules(&db, chrono::Utc::now()).await {
                        Ok(n) if n > 0 => tracing::debug!(fired = n, "scheduler tick fired runs"),
                        Ok(_) => {}
                        Err(e) => tracing::warn!(error = %e, "scheduler tick failed"),
                    }
                }
            }
        }
        tracing::info!("cron scheduler stopped");
    });
}

fn spawn_reaper(db: sqlx::PgPool, reap_after_secs: i64, cancel: CancellationToken) {
    // Sweep cadence: a third of the threshold, clamped so a small threshold does
    // not busy-loop and a large one still gets regular sweeps.
    let tick = Duration::from_secs((reap_after_secs / 3).clamp(15, 60) as u64);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tick);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // Skip the immediate first tick so we don't sweep before workers have a
        // chance to re-register on a coordinated restart.
        interval.tick().await;
        tracing::info!(
            reap_after_secs,
            tick_secs = tick.as_secs(),
            "worker reaper started"
        );
        loop {
            tokio::select! {
                () = cancel.cancelled() => break,
                _ = interval.tick() => {
                    match coveflow_queue::reap_lost_workers(&db, reap_after_secs).await {
                        Ok(o) if o.runs_failed > 0 || o.workers_removed > 0 => tracing::info!(
                            runs_failed = o.runs_failed,
                            workers_removed = o.workers_removed,
                            "reaped lost workers"
                        ),
                        Ok(_) => {}
                        Err(e) => tracing::warn!(error = %e, "worker reaper sweep failed"),
                    }
                }
            }
        }
        tracing::info!("worker reaper stopped");
    });
}

fn worker_config_from_env() -> Result<WorkerConfig> {
    let mut config = WorkerConfig::default();

    let worker_name = env_first(&["COVEFLOW_WORKER_NAME", "WORKER_NAME"]).ok_or_else(|| {
        anyhow!(
            "COVEFLOW_WORKER_NAME (or WORKER_NAME) must be set in worker/all mode — \
             multiple workers sharing a name will collide on the worker_ping primary key"
        )
    })?;
    config.worker_name = worker_name;

    if let Some(tags) = env_first(&["COVEFLOW_WORKER_TAGS", "WORKER_TAGS"]) {
        config.tags = parse_tags(&tags);
    }
    // Resource limits: each dimension is auto-detected at worker startup when its
    // env var is unset (None), clamped to detected when set; reservations carve
    // out headroom. Resolution happens in run_worker (after worker_dir is known),
    // so it is left to WorkerConfig here.
    config.total_cpus = env_parse_opt("COVEFLOW_WORKER_TOTAL_CPUS")?;
    config.total_memory_mb = env_parse_opt("COVEFLOW_WORKER_TOTAL_MEMORY_MB")?;
    config.total_disk_mb = env_parse_opt("COVEFLOW_WORKER_TOTAL_DISK_MB")?;
    config.reserved_cpus = env_parse("COVEFLOW_WORKER_RESERVED_CPUS", 0.0)?;
    // A negative reservation would make the worker advertise MORE than detected
    // (total - reserved); a non-finite one would silently zero capacity. Reject
    // both here so the misconfig fails loudly instead of corrupting the pool.
    if !config.reserved_cpus.is_finite() || config.reserved_cpus < 0.0 {
        return Err(anyhow!(
            "COVEFLOW_WORKER_RESERVED_CPUS must be a finite, non-negative number"
        ));
    }
    // memory/disk reservations are u64 — parsing already rejects negatives.
    config.reserved_memory_mb = env_parse("COVEFLOW_WORKER_RESERVED_MEMORY_MB", 0)?;
    config.reserved_disk_mb = env_parse("COVEFLOW_WORKER_RESERVED_DISK_MB", 0)?;
    config.default_run_timeout_secs = env_parse(
        "COVEFLOW_WORKER_DEFAULT_RUN_TIMEOUT_SECS",
        config.default_run_timeout_secs,
    )?;

    let poll_interval_secs = env_parse(
        "COVEFLOW_WORKER_POLL_INTERVAL_SECS",
        config.poll_interval.as_secs(),
    )?;
    config.poll_interval = Duration::from_secs(poll_interval_secs);

    config.worker_dir = env_first(&["COVEFLOW_WORKER_DIR", "WORKER_DIR"]);
    config.sandbox_mode = sandbox_mode_from_env()?;
    config.claim_concurrency = env_parse(
        "COVEFLOW_WORKER_CLAIM_CONCURRENCY",
        config.claim_concurrency,
    )?;

    Ok(config)
}

/// Bootstrap an instance admin from `COVEFLOW_INSTANCE_ADMIN_EMAIL` +
/// `COVEFLOW_INSTANCE_ADMIN_PASSWORD`. No-op when neither is set; errors if only
/// one is set (likely a misconfig).
async fn bootstrap_admin_from_env(db: &sqlx::PgPool) -> Result<()> {
    // Trim: K8s downward API / YAML / heredoc env values often carry a trailing
    // newline or spaces. An untrimmed email would pollute the account PK (lockout)
    // and untrimmed whitespace would be hashed into the password verbatim.
    let email = env_first(&["COVEFLOW_INSTANCE_ADMIN_EMAIL"]).map(|s| s.trim().to_owned());
    let password = env_first(&["COVEFLOW_INSTANCE_ADMIN_PASSWORD"]).map(|s| s.trim().to_owned());

    match (email, password) {
        (Some(email), Some(password)) => {
            // Minimal email-shape check: the email is the account PK and bootstrap
            // is create-only, so a typo'd value (e.g. "admin" with no @) would
            // persist and be unreachable by the email-based login until fixed by SQL.
            if !looks_like_email(&email) {
                return Err(anyhow!(
                    "COVEFLOW_INSTANCE_ADMIN_EMAIL does not look like an email address"
                ));
            }
            if password.chars().count() < 8 {
                return Err(anyhow!(
                    "COVEFLOW_INSTANCE_ADMIN_PASSWORD must be at least 8 characters"
                ));
            }
            coveflow_api::auth::bootstrap_admin(db, &email, &password)
                .await
                .map_err(|e| anyhow!("failed to bootstrap instance admin account: {e}"))
        }
        (None, None) => Ok(()),
        _ => Err(anyhow!(
            "COVEFLOW_INSTANCE_ADMIN_EMAIL and COVEFLOW_INSTANCE_ADMIN_PASSWORD \
             must both be set, or neither"
        )),
    }
}

/// Minimal email-shape validation: a non-empty local part and domain separated
/// by exactly one '@'. Deliberately permissive — just enough to catch typos like
/// a missing '@' before they become a permanent account PK.
fn looks_like_email(s: &str) -> bool {
    match s.split_once('@') {
        Some((local, domain)) => !local.is_empty() && !domain.is_empty() && !domain.contains('@'),
        None => false,
    }
}

fn sandbox_mode_from_env() -> Result<SandboxMode> {
    let raw = env_first(&["COVEFLOW_SANDBOX_MODE", "SANDBOX_MODE"]).ok_or_else(|| {
        anyhow!(
            "COVEFLOW_SANDBOX_MODE (or SANDBOX_MODE) must be set explicitly — \
             default 'none' would run untrusted user code directly on the host. \
             Set to 'none'/'dev' for local development, or 'nsjail' for production"
        )
    })?;
    match raw.trim().to_ascii_lowercase().as_str() {
        "none" | "dev" => Ok(SandboxMode::None),
        "nsjail" => Ok(SandboxMode::Nsjail(NsjailConfig::default())),
        "k8s" | "kubernetes" | "kubernetes_pod" => Err(anyhow!(
            "Kubernetes sandbox startup configuration is not wired yet"
        )),
        other => Err(anyhow!(
            "invalid sandbox mode '{other}', expected one of: none, nsjail"
        )),
    }
}

fn parse_tags(value: &str) -> Vec<String> {
    let tags: Vec<String> = value
        .split(',')
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .map(ToOwned::to_owned)
        .collect();

    if tags.is_empty() {
        vec!["default".to_string()]
    } else {
        tags
    }
}

fn env_first(names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() => Some(value),
        _ => None,
    })
}

/// Parse an optional env var: `Ok(None)` when unset/blank, `Err` when present
/// but unparseable. Used for resource limits where "unset" means auto-detect.
fn env_parse_opt<T>(name: &str) -> Result<Option<T>>
where
    T: FromStr,
    T::Err: std::fmt::Display,
{
    match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() => value
            .parse::<T>()
            .map(Some)
            .map_err(|e| anyhow!("invalid {name}='{value}': {e}")),
        _ => Ok(None),
    }
}

fn env_parse<T>(name: &str, default: T) -> Result<T>
where
    T: FromStr,
    T::Err: std::fmt::Display,
{
    let value = match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() => value,
        _ => return Ok(default),
    };

    value
        .parse::<T>()
        .map_err(|e| anyhow!("invalid {name}='{value}': {e}"))
}

fn env_parse_bool(name: &str, default: bool) -> Result<bool> {
    match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() => match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Ok(true),
            "0" | "false" | "no" | "off" => Ok(false),
            other => Err(anyhow!("invalid {name}='{other}', expected boolean")),
        },
        _ => Ok(default),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_mode(value: &str) -> RunMode {
        match value.parse::<RunMode>() {
            Ok(mode) => mode,
            Err(e) => panic!("failed to parse test mode '{value}': {e}"),
        }
    }

    #[test]
    fn parses_run_mode_aliases() {
        assert_eq!(parse_mode("api"), RunMode::Api);
        assert_eq!(parse_mode("server"), RunMode::Api);
        assert_eq!(parse_mode("worker"), RunMode::Worker);
        assert_eq!(parse_mode("all"), RunMode::All);
        assert_eq!(parse_mode("both"), RunMode::All);
    }

    #[test]
    fn parse_tags_filters_empty_items() {
        assert_eq!(parse_tags(" default, gpu ,, "), vec!["default", "gpu"]);
        assert_eq!(parse_tags(" , "), vec!["default"]);
    }
}
