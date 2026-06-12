use std::net::UdpSocket;
use std::sync::Arc;

use sqlx::PgPool;
use sysinfo::System;

use crate::occupancy::OccupancyTracker;
use crate::resource_manager::ResourceManager;
use crate::worker::WorkerConfig;

pub(crate) struct WorkerPingState {
    sys: System,
    occupancy: OccupancyTracker,
}

impl WorkerPingState {
    pub(crate) fn new() -> Self {
        let mut sys = System::new();
        sys.refresh_cpu_usage();
        sys.refresh_memory();
        Self {
            sys,
            occupancy: OccupancyTracker::new(),
        }
    }

    pub(crate) fn record_occupancy(&mut self, busy: bool) {
        self.occupancy.record(std::time::Instant::now(), busy);
    }

    fn refresh(&mut self) {
        self.sys.refresh_cpu_usage();
        self.sys.refresh_memory();
    }

    fn cpu_usage_percent(&self) -> f32 {
        let cpus = self.sys.cpus();
        if cpus.is_empty() {
            return 0.0;
        }
        cpus.iter().map(|c| c.cpu_usage()).sum::<f32>() / cpus.len() as f32
    }

    fn memory_usage_bytes(&self) -> i64 {
        self.sys.used_memory() as i64
    }

    fn disk_stats(&self) -> (i64, i64) {
        let disks = sysinfo::Disks::new_with_refreshed_list();
        let total = disks
            .iter()
            .map(|d| d.total_space() as i64)
            .max()
            .unwrap_or(0);
        let used = disks
            .iter()
            .map(|d| (d.total_space() - d.available_space()) as i64)
            .max()
            .unwrap_or(0);
        (total, used)
    }

    fn vcpus(&self) -> i32 {
        self.sys.cpus().len() as i32
    }

    fn memory_total_bytes(&self) -> i64 {
        self.sys.total_memory() as i64
    }
}

fn detect_ip() -> Option<String> {
    if let Ok(ip) = std::env::var("COVEFLOW_WORKER_IP") {
        if !ip.trim().is_empty() {
            return Some(ip);
        }
    }
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    socket.local_addr().ok().map(|a| a.ip().to_string())
}

#[tracing::instrument(
    name = "worker::init_ping",
    skip(db, state, rm, config),
    fields(worker_name = %config.worker_name)
)]
pub(crate) async fn init_ping(
    db: &PgPool,
    config: &WorkerConfig,
    display_name: &str,
    rm: &Arc<ResourceManager>,
    state: &mut WorkerPingState,
) {
    state.refresh();

    let ip = detect_ip();
    let sandbox = config.sandbox_mode.name();
    let (used_cpu, used_mem, used_disk) = rm.used();

    // Resolved capacity lives in the ResourceManager (single source of truth),
    // not the (possibly auto/None) WorkerConfig.
    let (total_cpus, total_memory_mb_u, total_disk_mb_u) = rm.totals();
    let total_memory_mb = total_memory_mb_u as i64;
    let total_disk_mb = total_disk_mb_u as i64;

    let vcpus = state.vcpus();
    let memory_total = state.memory_total_bytes();
    let (disk_total, disk_usage) = state.disk_stats();
    let cpu_usage = state.cpu_usage_percent();
    let memory_usage = state.memory_usage_bytes();

    let result = sqlx::query!(
        r#"
        INSERT INTO worker_ping (
            worker, display_name, ping_at, tags, ip, sandbox_mode,
            current_run_id, runs_completed,
            total_cpus, used_cpus, total_memory_mb, used_memory_mb,
            total_disk_mb, used_disk_mb,
            vcpus, memory_total, disk_total,
            cpu_usage_percent, memory_usage, disk_usage,
            occupancy_15s, occupancy_5m, occupancy_30m
        ) VALUES (
            $1, $17, now(), $2, $3, $4,
            NULL, 0,
            $5, $6, $7, $8,
            $9, $10,
            $11, $12, $13,
            $14, $15, $16,
            0.0, 0.0, 0.0
        )
        ON CONFLICT (worker) DO UPDATE SET
            display_name = $17,
            ping_at = now(),
            tags = $2,
            ip = $3,
            sandbox_mode = $4,
            current_run_id = NULL,
            runs_completed = 0,
            total_cpus = $5,
            used_cpus = $6,
            total_memory_mb = $7,
            used_memory_mb = $8,
            total_disk_mb = $9,
            used_disk_mb = $10,
            vcpus = $11,
            memory_total = $12,
            disk_total = $13,
            cpu_usage_percent = $14,
            memory_usage = $15,
            disk_usage = $16,
            occupancy_15s = 0.0,
            occupancy_5m = 0.0,
            occupancy_30m = 0.0
        "#,
        &config.worker_name,
        &config.tags as &[String],
        ip as Option<String>,
        sandbox,
        total_cpus as f32,
        used_cpu as f32,
        total_memory_mb,
        used_mem as i64,
        total_disk_mb,
        used_disk as i64,
        vcpus,
        memory_total,
        disk_total,
        cpu_usage,
        memory_usage,
        disk_usage,
        display_name,
    )
    .execute(db)
    .await;

    match result {
        Ok(_) => tracing::info!(worker_name = %config.worker_name, "init ping registered"),
        Err(e) => tracing::error!(error = %e, "init ping failed"),
    }
}

#[tracing::instrument(
    name = "worker::periodic_ping",
    skip(db, state, rm),
    fields(%worker_name),
    level = "debug"
)]
pub(crate) async fn periodic_ping(
    db: &PgPool,
    worker_name: &str,
    rm: &Arc<ResourceManager>,
    state: &mut WorkerPingState,
) {
    state.refresh();

    let (used_cpu, used_mem, used_disk) = rm.used();
    let cpu_usage = state.cpu_usage_percent();
    let memory_usage = state.memory_usage_bytes();
    let (_, disk_usage) = state.disk_stats();
    let occ_15s = state.occupancy.occupancy_15s();
    let occ_5m = state.occupancy.occupancy_5m();
    let occ_30m = state.occupancy.occupancy_30m();

    let result = sqlx::query!(
        r#"
        UPDATE worker_ping SET
            ping_at = now(),
            used_cpus = $2,
            used_memory_mb = $3,
            used_disk_mb = $4,
            cpu_usage_percent = $5,
            memory_usage = $6,
            disk_usage = $7,
            occupancy_15s = $8,
            occupancy_5m = $9,
            occupancy_30m = $10
        WHERE worker = $1
        "#,
        worker_name,
        used_cpu as f32,
        used_mem as i64,
        used_disk as i64,
        cpu_usage,
        memory_usage,
        disk_usage,
        occ_15s,
        occ_5m,
        occ_30m,
    )
    .execute(db)
    .await;

    match result {
        Ok(_) => tracing::debug!(worker_name, "periodic ping updated"),
        Err(e) => tracing::warn!(error = %e, "periodic ping failed"),
    }
}
