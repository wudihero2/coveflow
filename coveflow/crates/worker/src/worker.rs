use std::sync::Arc;

use coveflow_queue::{ActiveRun, claim_run, finish_run, unclaim_run};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;

use crate::error::{SandboxError, SandboxResult};
use crate::metrics::CoveflowMetrics;
use crate::ping::{WorkerPingState, init_ping, periodic_ping};
use crate::python::exec_python;
use crate::resource_detect::{
    ConfiguredResources, ReservedResources, detect_resources, resolve_resources,
};
use crate::resource_manager::{ResourceGuard, ResourceManager};
use crate::sandbox::{SandboxMode, SandboxResources, SandboxRouter, ScriptLang};

#[derive(Debug, Clone)]
pub struct WorkerConfig {
    pub worker_name: String,
    pub tags: Vec<String>,
    /// Configured capacity per dimension; `None` means auto-detect at startup
    /// (cgroup quota / host totals / free disk). Resolved in `run_worker`, so
    /// every construction path (env, tests, library embed) auto-detects rather
    /// than inheriting a hardcoded default.
    pub total_cpus: Option<f32>,
    pub total_memory_mb: Option<u64>,
    pub total_disk_mb: Option<u64>,
    /// Capacity held back from job scheduling (OS / co-located service headroom).
    pub reserved_cpus: f32,
    pub reserved_memory_mb: u64,
    pub reserved_disk_mb: u64,
    pub worker_dir: Option<String>,
    pub poll_interval: std::time::Duration,
    pub default_run_timeout_secs: u32,
    pub sandbox_mode: SandboxMode,
    pub claim_concurrency: usize,
    /// Master key for decrypting secrets to inject into runs. `None` disables
    /// injection (tests / a worker built without the key); production sets it
    /// from the fail-fast-validated `COVEFLOW_SECRET_KEY`.
    pub secret_key: Option<coveflow_types::crypto::SecretKey>,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            worker_name: "worker-default".to_string(),
            tags: vec!["default".to_string()],
            total_cpus: None,
            total_memory_mb: None,
            total_disk_mb: None,
            reserved_cpus: 0.0,
            reserved_memory_mb: 0,
            reserved_disk_mb: 0,
            worker_dir: None,
            poll_interval: std::time::Duration::from_secs(1),
            default_run_timeout_secs: 3600,
            sandbox_mode: SandboxMode::default(),
            claim_concurrency: 8,
            secret_key: None,
        }
    }
}

#[tracing::instrument(
    name = "worker::run_worker",
    skip(db, metrics, cancel),
    fields(
        worker_name = %config.worker_name,
        tags = ?config.tags,
        claim_concurrency = config.claim_concurrency,
    )
)]
pub async fn run_worker(
    db: PgPool,
    mut config: WorkerConfig,
    metrics: Arc<CoveflowMetrics>,
    cancel: CancellationToken,
) {
    // Identity vs display name: the configured name is what operators see, but a
    // worker's *identity* (worker_ping PK + run_queue.worker) must be unique per
    // process start. Otherwise a restart re-registers the same name with a fresh
    // heartbeat, so the dead process's still-`running` runs never look "lost" and
    // the reaper never reclaims them — their parent flows wait forever. A random
    // suffix makes the old row go stale and get reaped on the normal path.
    let display_name = config.worker_name.clone();
    config.worker_name = format!(
        "{display_name}-{}",
        &uuid::Uuid::new_v4().simple().to_string()[..6]
    );

    let worker_dir = config
        .worker_dir
        .clone()
        .unwrap_or_else(|| format!("/tmp/coveflow-worker-{}", config.worker_name));
    if let Err(e) = tokio::fs::create_dir_all(&worker_dir).await {
        tracing::error!(error = %e, path = %worker_dir, "failed to create worker directory");
        return;
    }

    // Verify nsjail binary at startup if nsjail mode is configured.
    #[cfg(target_os = "linux")]
    if let SandboxMode::Nsjail(ref nsjail_config) = config.sandbox_mode {
        match std::process::Command::new(&nsjail_config.nsjail_path)
            .arg("--help")
            .output()
        {
            Ok(output) if output.status.success() => {
                tracing::info!(path = %nsjail_config.nsjail_path, "nsjail binary verified");
            }
            Ok(_) | Err(_) => {
                tracing::error!(
                    path = %nsjail_config.nsjail_path,
                    "nsjail binary not usable — worker cannot run in nsjail mode"
                );
                return;
            }
        }
    }

    // Resolve advertised capacity now that worker_dir (used for disk detection)
    // is known: auto-detect any unset dimension, clamp configured overrides to
    // detected, then subtract reservations.
    let detected = detect_resources(std::path::Path::new(&worker_dir));
    let (total_cpus, total_memory_mb, total_disk_mb) = resolve_resources(
        ConfiguredResources {
            cpus: config.total_cpus,
            memory_mb: config.total_memory_mb,
            disk_mb: config.total_disk_mb,
        },
        detected,
        ReservedResources {
            cpus: config.reserved_cpus,
            memory_mb: config.reserved_memory_mb,
            disk_mb: config.reserved_disk_mb,
        },
    );
    tracing::info!(
        detected_cpus = detected.cpus,
        detected_memory_mb = detected.memory_mb,
        detected_disk_mb = detected.disk_mb,
        total_cpus,
        total_memory_mb,
        total_disk_mb,
        "worker resource limits resolved"
    );

    let rm = Arc::new(ResourceManager::new(
        total_cpus,
        total_memory_mb,
        total_disk_mb,
    ));

    metrics.resource_cpus_total.set(total_cpus as f64);
    metrics.resource_memory_total_mb.set(total_memory_mb as i64);

    // Init ping: register this worker in worker_ping
    let ping_state = Arc::new(tokio::sync::Mutex::new(WorkerPingState::new()));
    init_ping(
        &db,
        &config,
        &display_name,
        &rm,
        &mut *ping_state.lock().await,
    )
    .await;

    // Occupancy sampling task (1s) — lightweight, never blocked by DB
    let sample_cancel = cancel.clone();
    let sample_rm = Arc::clone(&rm);
    let sample_state = Arc::clone(&ping_state);
    let _sample_handle = tokio::spawn(async move {
        let interval = std::time::Duration::from_secs(1);
        loop {
            tokio::select! {
                () = tokio::time::sleep(interval) => {}
                () = sample_cancel.cancelled() => break,
            }
            let (used_cpu, _, _) = sample_rm.used();
            sample_state.lock().await.record_occupancy(used_cpu > 0.0);
        }
    });

    // DB heartbeat task (30s) — may be slow, won't block sampling
    let ping_cancel = cancel.clone();
    let ping_db = db.clone();
    let ping_rm = Arc::clone(&rm);
    let ping_worker_name = config.worker_name.clone();
    let ping_state_for_heartbeat = Arc::clone(&ping_state);
    let ping_handle = tokio::spawn(async move {
        let interval = std::time::Duration::from_secs(30);
        loop {
            tokio::select! {
                () = tokio::time::sleep(interval) => {}
                () = ping_cancel.cancelled() => break,
            }
            periodic_ping(
                &ping_db,
                &ping_worker_name,
                &ping_rm,
                &mut *ping_state_for_heartbeat.lock().await,
            )
            .await;
        }
    });

    // Gauge updater task
    let gauge_cancel = cancel.clone();
    let gauge_db = db.clone();
    let gauge_rm = Arc::clone(&rm);
    let gauge_metrics = metrics.clone();
    let gauge_handle = tokio::spawn(async move {
        let interval = std::time::Duration::from_secs(10);
        loop {
            tokio::select! {
                () = tokio::time::sleep(interval) => {}
                () = gauge_cancel.cancelled() => break,
            }
            update_cluster_gauges(&gauge_db, &gauge_metrics).await;
            let (used_cpu, used_mem, _) = gauge_rm.used();
            gauge_metrics.resource_cpus_used.set(used_cpu as f64);
            gauge_metrics.resource_memory_used_mb.set(used_mem as i64);
        }
    });

    tracing::info!(
        worker_dir = %worker_dir,
        poll_interval_secs = config.poll_interval.as_secs(),
        claim_concurrency = config.claim_concurrency,
        "worker started"
    );

    // Shared cancel registry: batch poller drives all running jobs
    let cancel_registry = Arc::new(CancelRegistry::new());

    // Cancel poll task: batch check every 500ms
    let cancel_poll_cancel = cancel.clone();
    let cancel_poll_registry = Arc::clone(&cancel_registry);
    let cancel_poll_db = db.clone();
    let cancel_poll_handle = tokio::spawn(async move {
        let active_interval = std::time::Duration::from_millis(500);
        let idle_interval = std::time::Duration::from_secs(2);
        loop {
            let run_ids = cancel_poll_registry.active_run_ids();
            let interval = if run_ids.is_empty() {
                idle_interval
            } else {
                active_interval
            };
            tokio::select! {
                () = tokio::time::sleep(interval) => {}
                () = cancel_poll_cancel.cancelled() => break,
            }
            if run_ids.is_empty() {
                continue;
            }
            let rows = sqlx::query!(
                "SELECT id, canceled_by FROM run_queue WHERE id = ANY($1) AND canceled_by IS NOT NULL",
                &run_ids,
            )
            .fetch_all(&cancel_poll_db)
            .await;

            if let Ok(rows) = rows {
                for r in rows {
                    if r.canceled_by.is_some() && cancel_poll_registry.cancel(r.id) {
                        tracing::info!(run_id = %r.id, "cancel detected via batch poll");
                    }
                }
            }
        }
    });

    // Spawn N claim loops
    let mut claim_handles = Vec::with_capacity(config.claim_concurrency);
    for loop_id in 0..config.claim_concurrency {
        let db = db.clone();
        let config = config.clone();
        let rm = Arc::clone(&rm);
        let metrics = metrics.clone();
        let cancel = cancel.clone();
        let worker_dir = worker_dir.clone();
        let cancel_registry = Arc::clone(&cancel_registry);

        claim_handles.push(tokio::spawn(claim_loop(
            loop_id,
            db,
            config,
            rm,
            metrics,
            cancel,
            worker_dir,
            cancel_registry,
        )));
    }

    // Wait for any claim loop to exit (they all exit on cancel)
    for handle in claim_handles {
        let _ = handle.await;
    }

    cancel.cancel();
    let _ = cancel_poll_handle.await;
    let _ = ping_handle.await;
    let _ = gauge_handle.await;

    // Cleanup worker directory (best-effort)
    if let Err(e) = tokio::fs::remove_dir_all(&worker_dir).await {
        tracing::debug!(error = %e, path = %worker_dir, "worker directory cleanup skipped");
    }

    tracing::info!("worker shutdown complete");
}

#[allow(clippy::too_many_arguments)]
async fn claim_loop(
    loop_id: usize,
    db: PgPool,
    config: WorkerConfig,
    rm: Arc<ResourceManager>,
    metrics: Arc<CoveflowMetrics>,
    cancel: CancellationToken,
    worker_dir: String,
    cancel_registry: Arc<CancelRegistry>,
) {
    let min_cpu: f32 = 0.1;
    let min_mem: u64 = 1;
    let min_disk: u64 = 1;

    loop {
        if cancel.is_cancelled() {
            break;
        }

        // Pre-acquire minimum resources before hitting DB.
        // This prevents multiple loops from over-claiming.
        let mut guard = loop {
            if cancel.is_cancelled() {
                return;
            }
            if let Some(g) = rm.try_acquire(min_cpu, min_mem, min_disk) {
                break g;
            }
            tokio::select! {
                () = rm.wait_for_release() => {}
                () = cancel.cancelled() => return,
            }
        };

        let (free_cpu, free_mem, free_disk) = rm.available();

        match claim_run(
            &db,
            &config.worker_name,
            &config.tags,
            free_cpu + min_cpu,
            (free_mem + min_mem) as i64,
            (free_disk + min_disk) as i64,
        )
        .await
        {
            Ok(Some(active_run)) => {
                let run_id = active_run.run.id;
                metrics.jobs_pulled_total.inc();

                // Resize guard from min to actual job resources
                if !guard.try_resize(
                    active_run.cpus,
                    active_run.memory_mb as u64,
                    active_run.disk_mb as u64,
                ) {
                    tracing::warn!(
                        loop_id,
                        run_id = %run_id,
                        "cannot resize to actual resources, unclaiming"
                    );
                    drop(guard);
                    if let Err(e) = unclaim_run(&db, run_id, &config.worker_name).await {
                        tracing::error!(error = %e, run_id = %run_id, "unclaim_run failed");
                        finish_run_with_error(
                            &db,
                            run_id,
                            "worker resource allocation failed after claim",
                            0,
                            0,
                        )
                        .await;
                    }
                    continue;
                }

                tracing::info!(
                    loop_id,
                    run_id = %run_id,
                    tag = %active_run.tag,
                    cpus = active_run.cpus,
                    memory_mb = active_run.memory_mb,
                    disk_mb = active_run.disk_mb,
                    "run claimed"
                );

                let (used_cpu, used_mem, _) = rm.used();
                metrics.resource_cpus_used.set(used_cpu as f64);
                metrics.resource_memory_used_mb.set(used_mem as i64);

                let run_dir = format!("{}/{}", worker_dir, run_id);
                let db_clone = db.clone();
                let default_timeout = config.default_run_timeout_secs;
                let sandbox_mode = config.sandbox_mode.clone();
                let secret_key = config.secret_key.clone();
                let m = metrics.clone();
                let cr = Arc::clone(&cancel_registry);

                tokio::spawn(async move {
                    execute_run(
                        db_clone,
                        active_run,
                        guard,
                        run_dir,
                        default_timeout,
                        sandbox_mode,
                        secret_key,
                        m,
                        cr,
                    )
                    .await;
                });

                continue;
            }
            Ok(None) => {
                // No job in queue — release pre-acquired resources and poll again
                drop(guard);
                tokio::select! {
                    () = tokio::time::sleep(config.poll_interval) => {}
                    () = cancel.cancelled() => break,
                }
            }
            Err(e) => {
                drop(guard);
                tracing::error!(loop_id, error = %e, "claim_run failed");
                tokio::select! {
                    () = tokio::time::sleep(config.poll_interval) => {}
                    () = cancel.cancelled() => break,
                }
            }
        }
    }
}

/// Mount a tmpfs at `path` with a size limit in bytes.
#[tracing::instrument(name = "worker::mount_tmpfs", fields(%path, size_bytes))]
async fn mount_tmpfs(path: &str, size_bytes: u64) -> Result<(), std::io::Error> {
    let output = tokio::process::Command::new("mount")
        .args([
            "-t",
            "tmpfs",
            "-o",
            &format!("size={size_bytes}"),
            "tmpfs",
            path,
        ])
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(std::io::Error::other(format!(
            "mount tmpfs failed: {stderr}"
        )));
    }
    Ok(())
}

/// Unmount a tmpfs at `path`.
#[tracing::instrument(name = "worker::unmount_tmpfs", fields(%path))]
async fn unmount_tmpfs(path: &str) -> Result<(), std::io::Error> {
    let output = tokio::process::Command::new("umount")
        .arg(path)
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(std::io::Error::other(format!("umount failed: {stderr}")));
    }
    Ok(())
}

#[tracing::instrument(name = "worker::cleanup_run_dir", fields(%run_dir, tmpfs_mounted))]
async fn cleanup_run_dir(run_dir: &str, tmpfs_mounted: bool) {
    if tmpfs_mounted {
        if let Err(e) = unmount_tmpfs(run_dir).await {
            tracing::warn!(error = %e, "tmpfs unmount failed, trying lazy unmount");
            if let Err(lazy_err) = tokio::process::Command::new("umount")
                .args(["-l", run_dir])
                .output()
                .await
            {
                tracing::debug!(error = %lazy_err, "lazy tmpfs unmount failed");
            }
        }
    }

    if let Err(e) = tokio::fs::remove_dir_all(run_dir).await {
        tracing::debug!(error = %e, "run directory cleanup failed");
    }
}

struct CancelRegistry {
    tokens: std::sync::Mutex<std::collections::HashMap<uuid::Uuid, CancellationToken>>,
}

impl CancelRegistry {
    fn new() -> Self {
        Self {
            tokens: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    fn register(&self, run_id: uuid::Uuid, token: CancellationToken) {
        self.tokens
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(run_id, token);
    }

    fn unregister(&self, run_id: uuid::Uuid) {
        self.tokens
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&run_id);
    }

    fn cancel(&self, run_id: uuid::Uuid) -> bool {
        if let Some(token) = self
            .tokens
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&run_id)
        {
            token.cancel();
            true
        } else {
            false
        }
    }

    fn active_run_ids(&self) -> Vec<uuid::Uuid> {
        self.tokens
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .keys()
            .copied()
            .collect()
    }
}

#[tracing::instrument(
    name = "worker::execute_run",
    skip(db, active_run, _guard, sandbox_mode, metrics, cancel_registry),
    fields(
        run_id = %active_run.run.id,
        tag = %active_run.tag,
        duration_ms = tracing::field::Empty,
        success = tracing::field::Empty,
    )
)]
#[allow(clippy::too_many_arguments)]
async fn execute_run(
    db: PgPool,
    active_run: ActiveRun,
    _guard: ResourceGuard,
    run_dir: String,
    default_timeout_secs: u32,
    sandbox_mode: SandboxMode,
    secret_key: Option<coveflow_types::crypto::SecretKey>,
    metrics: Arc<CoveflowMetrics>,
    cancel_registry: Arc<CancelRegistry>,
) {
    let run_id = active_run.run.id;
    let start = std::time::Instant::now();

    let wait_secs = (chrono::Utc::now() - active_run.scheduled_for)
        .num_milliseconds()
        .max(0) as f64
        / 1000.0;
    metrics.job_queue_wait_seconds.observe(wait_secs);

    // Validate resource values from DB (i32 -> u64 would silently wrap negatives)
    if active_run.cpus < 0.0 || active_run.memory_mb < 0 || active_run.disk_mb < 0 {
        tracing::error!(
            run_id = %run_id,
            cpus = active_run.cpus,
            memory_mb = active_run.memory_mb,
            disk_mb = active_run.disk_mb,
            "invalid resource values from DB, rejecting run"
        );
        finish_run_with_error(&db, run_id, "invalid resource values from DB", 0, 0).await;
        return;
    }

    // Three execution modes. Maintenance and flow runs are orchestrated in-process
    // (no sandbox); everything else runs user code in the sandbox.
    match active_run.run.kind {
        coveflow_types::RunKind::Maintenance => {
            run_maintenance(&db, &active_run.run, run_id, start).await;
        }
        coveflow_types::RunKind::Flow | coveflow_types::RunKind::FlowPreview => {
            run_flow_step(&db, run_id, start).await;
        }
        _ => {
            run_sandboxed(
                &db,
                active_run,
                run_dir,
                default_timeout_secs,
                sandbox_mode,
                secret_key,
                &metrics,
                &cancel_registry,
                start,
            )
            .await;
        }
    }
}

/// Maintenance runs bypass the sandbox entirely (SQL cleanup only). Records the
/// outcome on the surrounding `execute_run` span.
async fn run_maintenance(
    db: &PgPool,
    run: &coveflow_types::run::Run,
    run_id: uuid::Uuid,
    start: std::time::Instant,
) {
    let result = crate::maintenance::exec_maintenance(db, run).await;
    let duration_ms = start.elapsed().as_millis() as i32;
    let span = tracing::Span::current();
    span.record("duration_ms", duration_ms);

    match result {
        Ok(value) => {
            span.record("success", true);
            if let Err(e) = finish_run(db, run_id, true, value, duration_ms, 0, None).await {
                tracing::error!(error = %e, run_id = %run_id, "finish_run failed for maintenance");
            }
        }
        Err(e) => {
            span.record("success", false);
            tracing::error!(error = %e, run_id = %run_id, "maintenance task failed");
            let error_json = serde_json::json!({ "error": { "message": e.to_string() } });
            if let Err(fe) = finish_run(db, run_id, false, error_json, duration_ms, 0, None).await {
                tracing::error!(error = %fe, run_id = %run_id, "finish_run failed after maintenance error");
            }
        }
    }
}

/// Flow runs are orchestrated, not sandboxed: advance the flow one step (pushing
/// child runs), then either suspend (waiting on children) or finish.
async fn run_flow_step(db: &PgPool, run_id: uuid::Uuid, start: std::time::Instant) {
    let result = coveflow_queue::advance_flow(db, run_id).await;
    let duration_ms = start.elapsed().as_millis() as i32;
    let span = tracing::Span::current();
    span.record("duration_ms", duration_ms);
    match result {
        // Suspended: advance_flow already parked the flow run; nothing to finish.
        Ok(coveflow_queue::FlowProgress::Suspended) => {}
        Ok(coveflow_queue::FlowProgress::Completed { result }) => {
            span.record("success", true);
            if let Err(e) = finish_run(db, run_id, true, result, duration_ms, 0, None).await {
                tracing::error!(error = %e, run_id = %run_id, "finish_run failed for flow");
            }
        }
        Ok(coveflow_queue::FlowProgress::Failed { error }) => {
            span.record("success", false);
            if let Err(e) = finish_run(db, run_id, false, error, duration_ms, 0, None).await {
                tracing::error!(error = %e, run_id = %run_id, "finish_run failed for flow");
            }
        }
        Err(e) => {
            span.record("success", false);
            tracing::error!(error = %e, run_id = %run_id, "flow engine error");
            finish_run_with_error(
                db,
                run_id,
                &format!("flow engine error: {e}"),
                duration_ms,
                0,
            )
            .await;
        }
    }
}

/// Hash-only runs (e.g. flow child steps) carry no inline code; load the script
/// body from the `script` table into the run before sandboxing. Returns
/// `Err(message)` if the referenced script is missing or the load failed.
async fn hydrate_script_by_hash(
    db: &PgPool,
    run: &mut coveflow_types::run::Run,
) -> Result<(), String> {
    if run.raw_code.is_some() {
        return Ok(());
    }
    let Some(hash) = run.script_hash.clone() else {
        return Ok(()); // neither code nor hash; dispatch_run reports the empty run
    };
    match load_script_by_hash(db, &run.workspace_id, &hash).await {
        Ok(Some(script)) => {
            run.raw_code = Some(script.content);
            if run.language.is_none() {
                run.language = Some(script.language);
            }
            if run.requirements.is_empty() {
                run.requirements = script.requirements;
            }
            if run.custom_image.is_none() {
                run.custom_image = script.runtime;
            }
            Ok(())
        }
        Ok(None) => Err(format!("no script found for hash {hash}")),
        Err(e) => Err(format!("failed to load script by hash: {e}")),
    }
}

/// Full sandboxed execution: prepare the run dir + disk/resource limits, run the
/// code in the sandbox, persist the outcome, and clean up.
#[allow(clippy::too_many_arguments)]
async fn run_sandboxed(
    db: &PgPool,
    mut active_run: ActiveRun,
    run_dir: String,
    default_timeout_secs: u32,
    sandbox_mode: SandboxMode,
    secret_key: Option<coveflow_types::crypto::SecretKey>,
    metrics: &Arc<CoveflowMetrics>,
    cancel_registry: &Arc<CancelRegistry>,
    start: std::time::Instant,
) {
    let run_id = active_run.run.id;

    // Create run directory
    if let Err(e) = tokio::fs::create_dir_all(&run_dir).await {
        tracing::error!(error = %e, "failed to create run directory");
        finish_run_with_error(db, run_id, &format!("failed to create run dir: {e}"), 0, 0).await;
        return;
    }

    if let Err(msg) = hydrate_script_by_hash(db, &mut active_run.run).await {
        finish_run_with_error(db, run_id, &msg, 0, 0).await;
        return;
    }

    let disk_bytes = active_run.disk_mb as u64 * 1024 * 1024;
    let tmpfs_mounted = if disk_bytes == 0 {
        false
    } else if sandbox_mode.supports_disk_limit() {
        match mount_tmpfs(&run_dir, disk_bytes).await {
            Ok(()) => {
                tracing::debug!(run_dir = %run_dir, disk_bytes, "tmpfs mounted for disk limit");
                true
            }
            Err(e) => {
                tracing::error!(error = %e, "failed to mount tmpfs for disk limit");
                finish_run_with_error(db, run_id, &format!("failed to mount tmpfs: {e}"), 0, 0)
                    .await;
                cleanup_run_dir(&run_dir, false).await;
                return;
            }
        }
    } else {
        tracing::warn!(
            sandbox_mode = ?sandbox_mode,
            disk_mb = active_run.disk_mb,
            disk_bytes,
            "disk_mb requested but sandbox mode does not enforce disk limits; script can fill host disk"
        );
        false
    };

    let sandbox_resources = SandboxResources {
        cpu: active_run.cpus,
        memory_bytes: active_run.memory_mb as u64 * 1024 * 1024,
        disk_bytes,
        timeout_secs: active_run
            .run
            .timeout
            .map(|t| t as u32)
            .unwrap_or(default_timeout_secs),
    };

    let cancel_token = CancellationToken::new();
    cancel_registry.register(run_id, cancel_token.clone());

    // Airflow-style execution context, injected into the script (signature-aware).
    // Auxiliary: a build failure must not fail the run — fall back to an empty ctx.
    let run_context = match coveflow_queue::build_run_context(db, run_id).await {
        Ok(ctx) => serde_json::to_value(ctx).unwrap_or_else(|_| serde_json::json!({})),
        Err(e) => {
            tracing::warn!(error = %e, run_id = %run_id, "failed to build run context; using empty ctx");
            serde_json::json!({})
        }
    };

    // Decrypt the secrets this run's creator can read. Unlike the ctx, a failure
    // here fails the run: a decrypt error means key drift/tampering and must
    // surface rather than silently run with a missing secret. `None` key (tests /
    // no key configured) → no secrets.
    let secrets = match &secret_key {
        Some(key) => {
            match crate::secrets::resolve_secrets(
                db,
                key,
                &active_run.run.workspace_id,
                &active_run.run.created_by,
            )
            .await
            {
                Ok(s) => s,
                Err(e) => {
                    finish_run_with_error(db, run_id, &e.to_string(), 0, 0).await;
                    cleanup_run_dir(&run_dir, tmpfs_mounted).await;
                    cancel_registry.unregister(run_id);
                    return;
                }
            }
        }
        None => std::collections::HashMap::new(),
    };

    let result = dispatch_run(
        &active_run.run,
        &run_dir,
        &run_context,
        &secrets,
        sandbox_resources,
        cancel_token,
        &sandbox_mode,
    )
    .await;
    let duration_ms = start.elapsed().as_millis() as u64;

    cancel_registry.unregister(run_id);

    let span = tracing::Span::current();
    span.record("duration_ms", duration_ms);

    metrics
        .job_duration_seconds
        .observe(duration_ms as f64 / 1000.0);

    match result {
        Ok(value) => {
            span.record("success", true);
            tracing::info!(run_id = %run_id, duration_ms, "run completed successfully");
            metrics.jobs_completed_total.inc();

            if let Err(e) = finish_run(db, run_id, true, value, duration_ms as i32, 0, None).await {
                tracing::error!(error = %e, run_id = %run_id, "finish_run failed");
            }
        }
        Err(SandboxError::Canceled(reason)) => {
            span.record("success", false);
            tracing::info!(run_id = %run_id, duration_ms, reason = %reason, "run was canceled");
            metrics.jobs_completed_total.inc();
            metrics.jobs_canceled_total.inc();

            let error_json = serde_json::json!({
                "error": {
                    "message": format!("run canceled: {reason}"),
                    "type": "canceled",
                }
            });

            if let Err(e) =
                finish_run(db, run_id, false, error_json, duration_ms as i32, 0, None).await
            {
                tracing::error!(error = %e, run_id = %run_id, "finish_run failed after cancel");
            }
        }
        Err(e) => {
            span.record("success", false);
            tracing::error!(error = %e, run_id = %run_id, duration_ms, "run execution failed");
            metrics.jobs_completed_total.inc();
            metrics.jobs_failed_total.inc();
            if matches!(e, SandboxError::Timeout { .. }) {
                metrics.jobs_timeout_total.inc();
            }

            let error_json = serde_json::json!({
                "error": {
                    "message": e.to_string(),
                }
            });

            if let Err(fe) =
                finish_run(db, run_id, false, error_json, duration_ms as i32, 0, None).await
            {
                tracing::error!(error = %fe, run_id = %run_id, "finish_run failed after execution error");
            }
        }
    }

    cleanup_run_dir(&run_dir, tmpfs_mounted).await;
}

#[tracing::instrument(
    name = "worker::dispatch_run",
    skip(run, run_context, secrets, sandbox_resources, cancel_token, sandbox_mode),
    fields(
        run_id = %run.id,
        language = run.language.as_ref().map(|l| l.as_str()).unwrap_or("unknown"),
    )
)]
async fn dispatch_run(
    run: &coveflow_types::run::Run,
    run_dir: &str,
    run_context: &serde_json::Value,
    secrets: &std::collections::HashMap<String, String>,
    sandbox_resources: SandboxResources,
    cancel_token: CancellationToken,
    sandbox_mode: &SandboxMode,
) -> SandboxResult<serde_json::Value> {
    let code = match &run.raw_code {
        Some(code) => code.clone(),
        None => {
            // raw_code should have been hydrated from the script table in
            // execute_run for hash-only runs; reaching here means neither was set.
            return Err(SandboxError::Other(
                "run has neither raw_code nor script_hash".to_string(),
            ));
        }
    };

    let language = run
        .language
        .clone()
        .ok_or_else(|| SandboxError::Other("run has no language specified".to_string()))?;

    let router = SandboxRouter::new(sandbox_mode);
    let sandbox = router.select(&run.tag);

    match language {
        ScriptLang::Python3 => {
            exec_python(
                run,
                &code,
                run_dir,
                run_context,
                secrets,
                sandbox,
                sandbox_resources,
                cancel_token,
            )
            .await
        }
    }
}

#[tracing::instrument(name = "worker::finish_run_with_error", skip(db), fields(%run_id, %message))]
async fn finish_run_with_error(
    db: &PgPool,
    run_id: uuid::Uuid,
    message: &str,
    duration_ms: i32,
    memory_peak: i64,
) {
    let error_json = serde_json::json!({
        "error": { "message": message }
    });

    if let Err(e) = finish_run(
        db,
        run_id,
        false,
        error_json,
        duration_ms,
        memory_peak,
        None,
    )
    .await
    {
        tracing::error!(
            error = %e,
            run_id = %run_id,
            "finish_run failed while reporting error"
        );
    }
}

struct ScriptBody {
    content: String,
    language: coveflow_types::ScriptLang,
    requirements: Vec<String>,
    runtime: Option<String>,
}

async fn load_script_by_hash(
    db: &PgPool,
    workspace_id: &str,
    hash: &str,
) -> Result<Option<ScriptBody>, sqlx::Error> {
    let row = sqlx::query!(
        r#"SELECT content,
                  language AS "language: coveflow_types::ScriptLang",
                  requirements,
                  runtime
           FROM script
           WHERE workspace_id = $1 AND hash = $2"#,
        workspace_id,
        hash
    )
    .fetch_optional(db)
    .await?;

    Ok(row.map(|r| ScriptBody {
        content: r.content,
        language: r.language,
        requirements: r.requirements,
        runtime: r.runtime,
    }))
}

async fn update_cluster_gauges(db: &PgPool, m: &CoveflowMetrics) {
    let row = sqlx::query!(
        r#"SELECT
            COUNT(*) FILTER (WHERE NOT running) AS "pending!",
            COUNT(DISTINCT worker) FILTER (WHERE running) AS "active_workers!"
        FROM run_queue"#
    )
    .fetch_one(db)
    .await;

    if let Ok(r) = row {
        m.queue_depth.set(r.pending);
        m.active_workers.set(r.active_workers);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_worker_config_default() {
        let config = WorkerConfig::default();
        assert_eq!(config.worker_name, "worker-default");
        assert_eq!(config.tags, vec!["default"]);
        // Capacity defaults to auto-detect (None), not a hardcoded value, so any
        // construction path resolves real capacity at startup.
        assert!(config.total_cpus.is_none());
        assert!(config.total_memory_mb.is_none());
        assert!(config.total_disk_mb.is_none());
        assert_eq!(config.reserved_cpus, 0.0);
        assert_eq!(config.reserved_memory_mb, 0);
        assert_eq!(config.reserved_disk_mb, 0);
        assert!(config.worker_dir.is_none());
        assert_eq!(config.poll_interval, std::time::Duration::from_secs(1));
        assert_eq!(config.default_run_timeout_secs, 3600);
        assert_eq!(config.claim_concurrency, 8);
    }

    #[test]
    fn test_sandbox_resources_conversion() {
        let memory_mb: i32 = 512;
        let disk_mb: i32 = 1024;
        let cpus: f32 = 2.0;

        let resources = SandboxResources {
            cpu: cpus,
            memory_bytes: memory_mb as u64 * 1024 * 1024,
            disk_bytes: disk_mb as u64 * 1024 * 1024,
            timeout_secs: 3600,
        };

        assert!((resources.cpu - 2.0).abs() < f32::EPSILON);
        assert_eq!(resources.memory_bytes, 512 * 1024 * 1024);
        assert_eq!(resources.disk_bytes, 1024 * 1024 * 1024);
        assert_eq!(resources.timeout_secs, 3600);
    }

    #[test]
    fn test_resource_flow_claim_and_release() {
        let rm = Arc::new(ResourceManager::new(4.0, 8192, 102400));

        let guard = rm.try_acquire(1.0, 512, 1024);
        assert!(guard.is_some());

        let (cpus, mem, disk) = rm.available();
        assert!((cpus - 3.0).abs() < f32::EPSILON);
        assert_eq!(mem, 8192 - 512);
        assert_eq!(disk, 102400 - 1024);

        let guard2 = rm.try_acquire(2.0, 2048, 20480);
        assert!(guard2.is_some());

        let (cpus, mem, disk) = rm.available();
        assert!((cpus - 1.0).abs() < f32::EPSILON);
        assert_eq!(mem, 8192 - 512 - 2048);
        assert_eq!(disk, 102400 - 1024 - 20480);

        drop(guard);
        let (cpus, mem, disk) = rm.available();
        assert!((cpus - 2.0).abs() < f32::EPSILON);
        assert_eq!(mem, 8192 - 2048);
        assert_eq!(disk, 102400 - 20480);

        drop(guard2);
        let (cpus, mem, disk) = rm.available();
        assert!((cpus - 4.0).abs() < f32::EPSILON);
        assert_eq!(mem, 8192);
        assert_eq!(disk, 102400);
    }

    fn make_test_run(raw_code: Option<&str>, language: Option<&str>) -> coveflow_types::run::Run {
        coveflow_types::run::Run {
            id: uuid::Uuid::new_v4(),
            workspace_id: "test-ws".to_string(),
            kind: coveflow_types::RunKind::Script,
            script_hash: None,
            script_path: None,
            raw_code: raw_code.map(String::from),
            language: language.map(|l| {
                l.parse::<coveflow_types::ScriptLang>()
                    .expect("invalid language")
            }),
            args: Some(serde_json::json!({})),
            tag: "none".to_string(),
            parent_run: None,
            root_run: None,
            requirements: vec![],
            timeout: Some(30),
            custom_image: None,
            created_by: "test@example.com".to_string(),
            trace_id: None,
            span_id: None,
        }
    }

    #[tokio::test]
    async fn test_dispatch_run_python_raw_code() {
        let run = make_test_run(
            Some("def main():\n    return {\"status\": \"ok\"}"),
            Some("python3"),
        );

        let tmp = tempfile::tempdir().expect("failed to create temp dir");
        let run_dir = tmp.path().to_string_lossy().to_string();
        let resources = SandboxResources::default();

        let mode = SandboxMode::None;
        let result = dispatch_run(
            &run,
            &run_dir,
            &serde_json::json!({}),
            &std::collections::HashMap::new(),
            resources,
            CancellationToken::new(),
            &mode,
        )
        .await;

        match result {
            Ok(value) => {
                assert_eq!(value["status"], "ok");
            }
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("No such file or directory") || err_str.contains("not found") {
                    eprintln!("python3 not available, skipping test");
                    return;
                }
                panic!("unexpected error: {e}");
            }
        }
    }

    #[tokio::test]
    async fn test_dispatch_run_no_code() {
        let run = make_test_run(None, Some("python3"));

        let tmp = tempfile::tempdir().expect("failed to create temp dir");
        let run_dir = tmp.path().to_string_lossy().to_string();
        let resources = SandboxResources::default();

        let mode = SandboxMode::None;
        let result = dispatch_run(
            &run,
            &run_dir,
            &serde_json::json!({}),
            &std::collections::HashMap::new(),
            resources,
            CancellationToken::new(),
            &mode,
        )
        .await;
        assert!(result.is_err());

        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("neither raw_code nor script_hash"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_negative_resource_values_wrap() {
        let memory_mb: i32 = -1;
        let as_u64 = memory_mb as u64;
        assert_eq!(as_u64, u64::MAX);
    }

    #[test]
    fn test_dispatch_run_unsupported_language() {
        for candidate in ["javascript", "bash"] {
            let result = candidate.parse::<coveflow_types::ScriptLang>();
            assert!(result.is_err(), "{candidate} should be rejected");
            assert!(
                result.unwrap_err().contains("invalid script language"),
                "{candidate} should report invalid script language"
            );
        }
    }
}
