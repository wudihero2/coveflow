use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::metrics::histogram::{Histogram, exponential_buckets};
use prometheus_client::registry::Registry;

#[derive(Clone)]
pub struct CoveflowMetrics {
    // --- Cluster ---
    pub queue_depth: Gauge,
    pub active_workers: Gauge,

    // --- Worker ---
    pub jobs_pulled_total: Counter,
    pub jobs_completed_total: Counter,
    pub jobs_failed_total: Counter,
    pub jobs_canceled_total: Counter,
    pub jobs_timeout_total: Counter,
    pub job_duration_seconds: Histogram,
    pub job_queue_wait_seconds: Histogram,

    pub resource_cpus_total: Gauge<f64, std::sync::atomic::AtomicU64>,
    pub resource_cpus_used: Gauge<f64, std::sync::atomic::AtomicU64>,
    pub resource_memory_total_mb: Gauge,
    pub resource_memory_used_mb: Gauge,
}

impl CoveflowMetrics {
    pub fn new(registry: &mut Registry) -> Self {
        let job_duration_buckets: Vec<f64> = exponential_buckets(0.1, 2.0, 14).collect();

        let queue_depth = Gauge::default();
        registry.register(
            "queue_depth",
            "Current number of pending runs",
            queue_depth.clone(),
        );

        let active_workers = Gauge::default();
        registry.register(
            "active_workers",
            "Number of active workers",
            active_workers.clone(),
        );

        let jobs_pulled_total = Counter::default();
        registry.register(
            "jobs_pulled",
            "Total number of jobs pulled by workers",
            jobs_pulled_total.clone(),
        );

        let jobs_completed_total = Counter::default();
        registry.register(
            "jobs_completed",
            "Total number of completed jobs",
            jobs_completed_total.clone(),
        );

        let jobs_failed_total = Counter::default();
        registry.register(
            "jobs_failed",
            "Total number of failed jobs",
            jobs_failed_total.clone(),
        );

        let jobs_canceled_total = Counter::default();
        registry.register(
            "jobs_canceled",
            "Total number of canceled jobs",
            jobs_canceled_total.clone(),
        );

        let jobs_timeout_total = Counter::default();
        registry.register(
            "jobs_timeout",
            "Total number of timed-out jobs",
            jobs_timeout_total.clone(),
        );

        let job_duration_seconds = Histogram::new(job_duration_buckets.clone());
        registry.register(
            "job_duration_seconds",
            "Job execution duration in seconds",
            job_duration_seconds.clone(),
        );

        let job_queue_wait_seconds = Histogram::new(job_duration_buckets);
        registry.register(
            "job_queue_wait_seconds",
            "Time jobs spent waiting in the queue",
            job_queue_wait_seconds.clone(),
        );

        let resource_cpus_total = Gauge::<f64, _>::default();
        registry.register(
            "resource_cpus_total",
            "Total CPUs available to the worker",
            resource_cpus_total.clone(),
        );

        let resource_cpus_used = Gauge::<f64, _>::default();
        registry.register(
            "resource_cpus_used",
            "CPUs currently in use",
            resource_cpus_used.clone(),
        );

        let resource_memory_total_mb = Gauge::default();
        registry.register(
            "resource_memory_total_mb",
            "Total memory available to the worker (MB)",
            resource_memory_total_mb.clone(),
        );

        let resource_memory_used_mb = Gauge::default();
        registry.register(
            "resource_memory_used_mb",
            "Memory currently in use (MB)",
            resource_memory_used_mb.clone(),
        );

        Self {
            queue_depth,
            active_workers,
            jobs_pulled_total,
            jobs_completed_total,
            jobs_failed_total,
            jobs_canceled_total,
            jobs_timeout_total,
            job_duration_seconds,
            job_queue_wait_seconds,
            resource_cpus_total,
            resource_cpus_used,
            resource_memory_total_mb,
            resource_memory_used_mb,
        }
    }

    pub fn noop() -> Self {
        let buckets: Vec<f64> = exponential_buckets(0.1, 2.0, 14).collect();
        Self {
            queue_depth: Gauge::default(),
            active_workers: Gauge::default(),
            jobs_pulled_total: Counter::default(),
            jobs_completed_total: Counter::default(),
            jobs_failed_total: Counter::default(),
            jobs_canceled_total: Counter::default(),
            jobs_timeout_total: Counter::default(),
            job_duration_seconds: Histogram::new(buckets.clone()),
            job_queue_wait_seconds: Histogram::new(buckets),
            resource_cpus_total: Gauge::default(),
            resource_cpus_used: Gauge::default(),
            resource_memory_total_mb: Gauge::default(),
            resource_memory_used_mb: Gauge::default(),
        }
    }
}
