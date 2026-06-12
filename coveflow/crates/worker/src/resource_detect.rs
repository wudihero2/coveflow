//! Worker resource auto-detection and limit resolution.
//!
//! On startup a worker decides how much CPU / memory / disk it may hand out to
//! jobs. Each dimension may be configured explicitly; when omitted it is
//! detected from the environment:
//!
//! - **CPU / memory**: cgroup v2 quota when present (so a container sees its
//!   limit, not the host's), otherwise host totals via `sysinfo`.
//! - **disk**: free space on the filesystem backing the worker's job directory.
//!
//! Configured values that exceed what is actually available are clamped down
//! with a warning — a worker never advertises more than it can back, so the
//! scheduler cannot overcommit and OOM the host.
//!
//! A configured `0` (or negative) is treated as "auto-detect", NOT as a hard
//! zero: it cannot be used to fence a worker off a dimension. To keep a worker
//! from offering a dimension, set a small positive cap (or reserve it) instead.
//! On a host running multiple workers, set explicit caps on every dimension —
//! otherwise each worker auto-detects and advertises the host totals, letting
//! the scheduler overcommit the shared machine.

use std::path::Path;

use sysinfo::System;

const MB: u64 = 1024 * 1024;

/// Resources detected as available to this worker (after cgroup quota, before
/// any operator reservation).
#[derive(Debug, Clone, Copy)]
pub struct DetectedResources {
    pub cpus: f32,
    pub memory_mb: u64,
    pub disk_mb: u64,
}

/// Operator-configured caps. `None` on a dimension means "auto-detect".
#[derive(Debug, Clone, Copy, Default)]
pub struct ConfiguredResources {
    pub cpus: Option<f32>,
    pub memory_mb: Option<u64>,
    pub disk_mb: Option<u64>,
}

/// Resources permanently held back from job scheduling (OS headroom, or a
/// co-located API server on a shared host).
#[derive(Debug, Clone, Copy, Default)]
pub struct ReservedResources {
    pub cpus: f32,
    pub memory_mb: u64,
    pub disk_mb: u64,
}

/// Detect the resources available to a worker whose job directory is `job_dir`.
pub fn detect_resources(job_dir: &Path) -> DetectedResources {
    let mut sys = System::new();
    sys.refresh_cpu_usage();
    sys.refresh_memory();

    DetectedResources {
        cpus: detect_cpus(&sys),
        memory_mb: detect_memory_mb(&sys),
        disk_mb: detect_disk_mb(job_dir),
    }
}

/// Resolve the final `(cpus, memory_mb, disk_mb)` a worker advertises.
///
/// For each dimension: take the configured value (or the detected value when
/// unset), clamp it down to the detected ceiling, then subtract the reservation.
/// Pure and total-order — covered directly by unit tests.
pub fn resolve_resources(
    configured: ConfiguredResources,
    detected: DetectedResources,
    reserved: ReservedResources,
) -> (f32, u64, u64) {
    let cpus = resolve_cpus(configured.cpus, detected.cpus, reserved.cpus);
    let memory_mb = resolve_u64(
        configured.memory_mb,
        detected.memory_mb,
        reserved.memory_mb,
        "memory_mb",
    );
    let disk_mb = resolve_u64(
        configured.disk_mb,
        detected.disk_mb,
        reserved.disk_mb,
        "disk_mb",
    );
    (cpus, memory_mb, disk_mb)
}

fn resolve_cpus(configured: Option<f32>, detected: f32, reserved: f32) -> f32 {
    let total = match configured {
        // Guard against NaN / inf from a malformed COVEFLOW_WORKER_TOTAL_CPUS:
        // f32::from_str accepts "nan"/"inf", and NaN comparisons are all false,
        // so without this it would silently fall through and advertise 0.
        Some(v) if !v.is_finite() => {
            tracing::warn!(
                configured = v,
                detected,
                dim = "cpus",
                "configured worker capacity is not a finite number; using detected"
            );
            detected
        }
        // A configured 0 (or negative) would otherwise pass through verbatim and
        // produce a worker that never claims a job — a silent ghost. Treat it like
        // the NaN case: warn and fall back to detected. NOTE this means 0 cannot
        // fence the worker off CPU; use a small positive cap or a reservation.
        Some(v) if v <= 0.0 => {
            tracing::warn!(
                configured = v,
                detected,
                dim = "cpus",
                "configured worker capacity is <= 0; 0 cannot fence a dimension to zero, \
                 ignoring and advertising the detected capacity instead"
            );
            detected
        }
        Some(v) if v > detected => {
            tracing::warn!(
                configured = v,
                detected,
                dim = "cpus",
                "configured worker capacity exceeds detected; clamping to detected"
            );
            detected
        }
        Some(v) => v,
        None => detected,
    };
    // Clamp reserved into [0, total] defensively: a negative/NaN reservation must
    // never let the worker advertise MORE than detected.
    let reserved = if reserved.is_finite() {
        reserved.max(0.0)
    } else {
        0.0
    };
    (total - reserved).max(0.0)
}

fn resolve_u64(configured: Option<u64>, detected: u64, reserved: u64, dim: &str) -> u64 {
    let total = match configured {
        // A configured 0 means "auto-detect", not "advertise nothing": it cannot
        // fence the worker off this dimension. To hold a dimension back use a
        // small positive cap or a reservation. (See resolve_cpus for the symmetry.)
        Some(0) => {
            tracing::warn!(
                detected,
                dim,
                "configured worker capacity is 0; 0 cannot fence a dimension to zero, \
                 ignoring and advertising the detected capacity instead"
            );
            detected
        }
        Some(v) if v > detected => {
            tracing::warn!(
                configured = v,
                detected,
                dim,
                "configured worker capacity exceeds detected; clamping to detected"
            );
            detected
        }
        Some(v) => v,
        None => detected,
    };
    total.saturating_sub(reserved)
}

fn detect_cpus(sys: &System) -> f32 {
    let physical = sys.cpus().len().max(1) as f32;
    #[cfg(target_os = "linux")]
    if let Some(cpus) = cgroup_cpu_limit() {
        // A cgroup quota can be misconfigured larger than the host (or absurd);
        // a worker can never use more than the physical cores, so cap there.
        return cpus.min(physical);
    }
    physical
}

fn detect_memory_mb(sys: &System) -> u64 {
    let physical = (sys.total_memory() / MB).max(1);
    #[cfg(target_os = "linux")]
    if let Some(mb) = cgroup_memory_limit_mb() {
        // Likewise cap a cgroup limit at physical RAM.
        return mb.min(physical);
    }
    physical
}

/// Free space (MB) on the filesystem that backs `job_dir`. Prefers the disk
/// whose mount point is the longest prefix of `job_dir`. When nothing matches
/// (e.g. overlay/bind mounts absent from the table) it falls back to the root
/// filesystem rather than an unrelated volume, and warns — picking the largest
/// arbitrary disk would advertise capacity the worker can't actually use.
fn detect_disk_mb(job_dir: &Path) -> u64 {
    let disks = sysinfo::Disks::new_with_refreshed_list();

    if let Some(d) = disks
        .iter()
        .filter(|d| job_dir.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len())
    {
        return (d.available_space() / MB).max(1);
    }

    if let Some(root) = disks.iter().find(|d| d.mount_point() == Path::new("/")) {
        tracing::warn!(
            job_dir = %job_dir.display(),
            "no mount point matches job dir; using root filesystem for disk capacity"
        );
        return (root.available_space() / MB).max(1);
    }

    // Truly undetectable (e.g. no mount table access): advertise 0 and warn,
    // rather than a misleading 1 MB that looks like a real reading.
    tracing::warn!(
        job_dir = %job_dir.display(),
        disk_count = disks.list().len(),
        "could not determine disk capacity for job dir; advertising 0 MB"
    );
    0
}

/// Parse cgroup v2 `cpu.max` (`"<quota> <period>"`, both microseconds).
/// `"max"` quota means unlimited → `None` (caller falls back to sysinfo).
#[cfg(target_os = "linux")]
fn cgroup_cpu_limit() -> Option<f32> {
    let content = std::fs::read_to_string("/sys/fs/cgroup/cpu.max").ok()?;
    let mut parts = content.split_whitespace();
    let quota = parts.next()?;
    if quota == "max" {
        return None;
    }
    let quota: f64 = quota.parse().ok()?;
    let period: f64 = parts.next()?.parse().ok()?;
    if period <= 0.0 {
        return None;
    }
    let cpus = (quota / period) as f32;
    (cpus.is_finite() && cpus > 0.0).then_some(cpus)
}

/// Parse cgroup v2 `memory.max` (bytes, or `"max"` for unlimited).
#[cfg(target_os = "linux")]
fn cgroup_memory_limit_mb() -> Option<u64> {
    let content = std::fs::read_to_string("/sys/fs/cgroup/memory.max").ok()?;
    let trimmed = content.trim();
    if trimmed == "max" {
        return None;
    }
    let bytes: u64 = trimmed.parse().ok()?;
    Some((bytes / MB).max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detected(cpus: f32, memory_mb: u64, disk_mb: u64) -> DetectedResources {
        DetectedResources {
            cpus,
            memory_mb,
            disk_mb,
        }
    }

    #[test]
    fn omitted_config_uses_detected_minus_reserved() {
        let (cpus, mem, disk) = resolve_resources(
            ConfiguredResources::default(),
            detected(8.0, 32768, 102400),
            ReservedResources {
                cpus: 1.0,
                memory_mb: 1024,
                disk_mb: 0,
            },
        );
        assert!((cpus - 7.0).abs() < f32::EPSILON);
        assert_eq!(mem, 31744);
        assert_eq!(disk, 102400);
    }

    #[test]
    fn configured_below_detected_is_used_verbatim() {
        let (cpus, mem, disk) = resolve_resources(
            ConfiguredResources {
                cpus: Some(2.0),
                memory_mb: Some(4096),
                disk_mb: Some(10240),
            },
            detected(8.0, 32768, 102400),
            ReservedResources::default(),
        );
        assert!((cpus - 2.0).abs() < f32::EPSILON);
        assert_eq!(mem, 4096);
        assert_eq!(disk, 10240);
    }

    #[test]
    fn configured_above_detected_is_clamped() {
        let (cpus, mem, disk) = resolve_resources(
            ConfiguredResources {
                cpus: Some(999.0),
                memory_mb: Some(999_999),
                disk_mb: Some(999_999),
            },
            detected(4.0, 8192, 51200),
            ReservedResources::default(),
        );
        assert!((cpus - 4.0).abs() < f32::EPSILON);
        assert_eq!(mem, 8192);
        assert_eq!(disk, 51200);
    }

    #[test]
    fn non_finite_configured_cpus_falls_back_to_detected() {
        // Malformed COVEFLOW_WORKER_TOTAL_CPUS=NaN/inf must not silently become 0.
        for bad in [f32::NAN, f32::INFINITY] {
            let (cpus, _, _) = resolve_resources(
                ConfiguredResources {
                    cpus: Some(bad),
                    memory_mb: None,
                    disk_mb: None,
                },
                detected(8.0, 32768, 102400),
                ReservedResources::default(),
            );
            assert!((cpus - 8.0).abs() < f32::EPSILON, "bad={bad}");
        }
    }

    #[test]
    fn configured_zero_or_negative_falls_back_to_detected() {
        // A 0/negative override must not produce a ghost worker that never claims.
        let (cpus, mem, disk) = resolve_resources(
            ConfiguredResources {
                cpus: Some(0.0),
                memory_mb: Some(0),
                disk_mb: Some(0),
            },
            detected(8.0, 32768, 102400),
            ReservedResources::default(),
        );
        assert!((cpus - 8.0).abs() < f32::EPSILON);
        assert_eq!(mem, 32768);
        assert_eq!(disk, 102400);

        let (cpus_neg, _, _) = resolve_resources(
            ConfiguredResources {
                cpus: Some(-1.0),
                memory_mb: None,
                disk_mb: None,
            },
            detected(4.0, 8192, 51200),
            ReservedResources::default(),
        );
        assert!((cpus_neg - 4.0).abs() < f32::EPSILON);
    }

    #[test]
    fn negative_or_nonfinite_reserved_never_over_advertises() {
        for bad in [-2.0, f32::NAN, f32::INFINITY] {
            let (cpus, _, _) = resolve_resources(
                ConfiguredResources::default(),
                detected(8.0, 32768, 102400),
                ReservedResources {
                    cpus: bad,
                    memory_mb: 0,
                    disk_mb: 0,
                },
            );
            assert!(
                cpus <= 8.0,
                "must not exceed detected; bad={bad} got {cpus}"
            );
            assert!(cpus >= 0.0, "must not be negative; bad={bad} got {cpus}");
        }
    }

    #[test]
    fn reservation_never_underflows() {
        let (cpus, mem, disk) = resolve_resources(
            ConfiguredResources::default(),
            detected(1.0, 512, 1024),
            ReservedResources {
                cpus: 4.0,
                memory_mb: 4096,
                disk_mb: 8192,
            },
        );
        assert_eq!(cpus, 0.0);
        assert_eq!(mem, 0);
        assert_eq!(disk, 0);
    }
}
