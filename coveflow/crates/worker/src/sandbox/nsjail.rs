use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use super::{Sandbox, SandboxContext, SandboxOutput, run_child};
use crate::config::{NsjailConfig, RuntimeEntry};
use crate::error::{SandboxError, SandboxResult};

/// nsjail exit code when time limit is reached.
const NSJAIL_EXIT_TIME_LIMIT: i32 = 109;

/// Post-exit check: nsjail uses exit code 109 to signal time limit exceeded.
fn check_nsjail_exit(exit_code: i32, timeout_secs: u32) -> Option<SandboxError> {
    if exit_code == NSJAIL_EXIT_TIME_LIMIT {
        tracing::warn!(timeout_secs, "nsjail time limit reached (exit 109)");
        Some(SandboxError::Timeout(timeout_secs))
    } else {
        None
    }
}

pub(crate) struct NsjailSandbox {
    config: NsjailConfig,
}

impl NsjailSandbox {
    pub fn new(config: NsjailConfig) -> Self {
        Self { config }
    }

    /// Resolve the runtime entry from custom_image or fall back to default
    /// when none was requested. An explicit custom_image that is not in the
    /// catalog is a hard error — silently falling back masks drift between
    /// the API allowlist and the worker catalog.
    fn resolve_runtime(&self, custom_image: Option<&str>) -> SandboxResult<&RuntimeEntry> {
        if let Some(key) = custom_image {
            return self.config.catalog.runtimes.get(key).ok_or_else(|| {
                SandboxError::Other(format!("requested runtime '{key}' not found in catalog"))
            });
        }

        self.config
            .catalog
            .runtimes
            .get(&self.config.catalog.default_runtime)
            .ok_or_else(|| {
                SandboxError::Other(format!(
                    "default runtime '{}' not found in catalog",
                    self.config.catalog.default_runtime
                ))
            })
    }

    /// Build the full nsjail command-line arguments.
    fn build_nsjail_args(&self, ctx: &SandboxContext) -> SandboxResult<Vec<String>> {
        let runtime = self.resolve_runtime(ctx.custom_image.as_deref())?;
        let mut args = vec![
            // Suppress nsjail's [I] info messages on stderr. They would
            // otherwise be interleaved with the child's stderr output and
            // captured as "user output". Warnings and errors still come
            // through, which the worker logs as pip/python errors.
            "--really_quiet".to_string(),
            // Execution mode: one-shot
            "--mode".to_string(),
            "o".to_string(),
            // Chroot to the runtime rootfs
            "--chroot".to_string(),
            runtime.rootfs.clone(),
            // User/group mapping
            "--user".to_string(),
            self.config.uid.to_string(),
            "--group".to_string(),
            self.config.gid.to_string(),
            // Time limit
            "--time_limit".to_string(),
            ctx.timeout_secs.to_string(),
        ];

        // Cgroup-based resource limits (memory, CPU, PIDs).
        // Only enable cgroups when at least one limit is set; otherwise
        // disable CLONE_NEWCGROUP so nsjail works in environments where
        // the cgroup filesystem may not be fully writable.
        let use_cgroups = ctx.resource_limits.memory_bytes > 0
            || ctx.resource_limits.cpu > 0.0
            || self.config.cgroup_pids_max > 0;

        if use_cgroups {
            args.push("--detect_cgroupv2".to_string());

            if ctx.resource_limits.memory_bytes > 0 {
                args.push("--cgroup_mem_max".to_string());
                args.push(ctx.resource_limits.memory_bytes.to_string());
                // Also limit swap to prevent exceeding memory via swap
                args.push("--cgroup_mem_swap_max".to_string());
                args.push("0".to_string());
            }
            if ctx.resource_limits.cpu > 0.0 {
                let cpu_ms_per_sec = (ctx.resource_limits.cpu * 1000.0) as u64;
                args.push("--cgroup_cpu_ms_per_sec".to_string());
                args.push(cpu_ms_per_sec.to_string());
            }
            if self.config.cgroup_pids_max > 0 {
                args.push("--cgroup_pids_max".to_string());
                args.push(self.config.cgroup_pids_max.to_string());
            }
        } else {
            args.push("--disable_clone_newcgroup".to_string());
        }

        // Network isolation. Per-context allow_network overrides the config
        // default: trusted stages (e.g. pip install) opt in to host network;
        // user script execution stays isolated. nsjail creates a new netns by
        // default — passing --disable_clone_newnet shares the host netns.
        let isolate_net = self.config.clone_newnet && !ctx.allow_network;
        if !isolate_net {
            args.push("--disable_clone_newnet".to_string());
        } else {
            // Inside the new netns, disable loopback so user code cannot reach
            // host-local services via 127.0.0.1 even if the jail were leaky.
            args.push("--iface_no_lo".to_string());
        }

        // Seccomp policy (Kafel). Empty string disables; default denies a
        // curated set of high-risk syscalls (mount/ptrace/bpf/...).
        if !self.config.seccomp_policy.is_empty() {
            args.push("--seccomp_string".to_string());
            args.push(self.config.seccomp_policy.clone());
        }

        // File descriptor limit (prevent fd exhaustion attacks)
        args.push("--rlimit_nofile".to_string());
        args.push("10000".to_string());

        // Single-file size limit (RLIMIT_FSIZE). nsjail defaults to 1 MB which
        // is far too small — pip wheel downloads alone exceed it. Scale to the
        // run's disk_mb so a single file can be as large as the disk quota.
        // Falls back to 1 GB when no disk limit is set (e.g. None sandbox).
        let disk_mb = if ctx.resource_limits.disk_bytes > 0 {
            ctx.resource_limits.disk_bytes / (1024 * 1024)
        } else {
            1024
        };
        args.push("--rlimit_fsize".to_string());
        args.push(disk_mb.to_string());

        // Working directory inside jail
        args.push("--cwd".to_string());
        args.push(self.config.work_mount.clone());

        // Bind-mount run_dir to /work.
        // Disk limit is enforced by mounting run_dir as tmpfs on the host
        // (done in worker.rs before execution), so the bind-mount inherits
        // the size limit automatically.
        args.push("--bindmount".to_string());
        args.push(format!("{}:{}", ctx.run_dir, self.config.work_mount));

        // Extra read-only bind mounts
        for mount in &self.config.extra_bind_mounts_ro {
            args.push("--bindmount_ro".to_string());
            args.push(mount.clone());
        }

        // Environment variables
        for (key, value) in &ctx.env {
            args.push("--env".to_string());
            args.push(format!("{key}={value}"));
        }

        // Separator between nsjail args and the command
        args.push("--".to_string());

        // The actual command to run inside the jail
        args.push(runtime.command.clone());

        // Command arguments
        args.extend(ctx.args.iter().cloned());

        Ok(args)
    }
}

#[async_trait]
impl Sandbox for NsjailSandbox {
    #[tracing::instrument(
        name = "sandbox.execute",
        skip(self, ctx, cancel),
        fields(
            run_id = %ctx.run_id,
            sandbox_mode = "nsjail",
            language = ?ctx.language,
            timeout_secs = ctx.timeout_secs,
            exit_code = tracing::field::Empty,
            duration_ms = tracing::field::Empty,
            memory_peak_bytes = tracing::field::Empty,
        )
    )]
    async fn execute(
        &self,
        ctx: &SandboxContext,
        cancel: CancellationToken,
    ) -> SandboxResult<SandboxOutput> {
        let nsjail_args = self.build_nsjail_args(ctx)?;

        tracing::debug!(
            nsjail_path = %self.config.nsjail_path,
            args = ?nsjail_args,
            "spawning nsjail process"
        );

        let mut cmd = tokio::process::Command::new(&self.config.nsjail_path);
        cmd.args(&nsjail_args);

        // Use a generous outer timeout beyond the nsjail --time_limit.
        // nsjail handles its own timeout internally; this is a safety net.
        let outer_timeout = std::time::Duration::from_secs(ctx.timeout_secs as u64 + 10);

        run_child(
            cmd,
            ctx,
            cancel,
            outer_timeout,
            "nsjail",
            Some(check_nsjail_exit),
        )
        .await
    }

    async fn health_check(&self) -> SandboxResult<()> {
        // Verify nsjail binary exists and is executable
        let nsjail_path = std::path::Path::new(&self.config.nsjail_path);
        if !nsjail_path.exists() {
            return Err(SandboxError::Other(format!(
                "nsjail binary not found at: {}",
                self.config.nsjail_path
            )));
        }

        // Verify at least the default runtime rootfs exists
        let default_runtime = self
            .config
            .catalog
            .runtimes
            .get(&self.config.catalog.default_runtime);

        if let Some(entry) = default_runtime {
            let rootfs_path = std::path::Path::new(&entry.rootfs);
            if !rootfs_path.exists() {
                return Err(SandboxError::Other(format!(
                    "default rootfs not found at: {}",
                    entry.rootfs
                )));
            }
        } else {
            return Err(SandboxError::Other(format!(
                "default runtime '{}' not found in catalog",
                self.config.catalog.default_runtime
            )));
        }

        Ok(())
    }

    fn name(&self) -> &str {
        "nsjail"
    }

    fn supports_resource_limits(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{NsjailConfig, RuntimeCatalog, default_seccomp_denied_syscalls};

    /// Returns true if nsjail binary is available on this system.
    fn nsjail_available() -> bool {
        std::process::Command::new("nsjail")
            .arg("--help")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Skip the test if nsjail is not installed (e.g. on macOS).
    macro_rules! require_nsjail {
        () => {
            if !nsjail_available() {
                eprintln!("nsjail not found, skipping test");
                return;
            }
        };
    }

    fn runtime_entry(result: SandboxResult<&RuntimeEntry>) -> &RuntimeEntry {
        match result {
            Ok(entry) => entry,
            Err(err) => panic!("runtime should resolve: {err}"),
        }
    }

    fn nsjail_args(result: SandboxResult<Vec<String>>) -> Vec<String> {
        match result {
            Ok(args) => args,
            Err(err) => panic!("nsjail args should build: {err}"),
        }
    }

    /// Catalog runtimes whose rootfs is actually present on this host.
    ///
    /// The seccomp policy is a fixed kernel filter, identical across runtimes, so
    /// probing one installed runtime exercises it fully. We filter to installed
    /// rootfs because CI prepares only a subset (e.g. the cgroup/seccomp job sets
    /// up just the default python:3.12); probing a runtime with no rootfs would
    /// make nsjail fail to launch the child rather than test the policy.
    fn default_runtime_names() -> Vec<String> {
        let mut runtimes = RuntimeCatalog::default()
            .runtimes
            .into_iter()
            .filter(|(_, entry)| std::path::Path::new(&entry.rootfs).is_dir())
            .map(|(name, _)| name)
            .collect::<Vec<_>>();
        runtimes.sort();
        runtimes
    }

    #[derive(Debug, Clone, Copy)]
    struct SeccompSyscallProbe {
        policy_name: &'static str,
        syscall_name: &'static str,
        syscall_number: i64,
    }

    #[cfg(target_arch = "aarch64")]
    fn default_seccomp_syscall_probes() -> Vec<SeccompSyscallProbe> {
        vec![
            SeccompSyscallProbe {
                policy_name: "mount",
                syscall_name: "mount",
                syscall_number: 40,
            },
            SeccompSyscallProbe {
                policy_name: "umount",
                syscall_name: "umount2",
                syscall_number: 39,
            },
            SeccompSyscallProbe {
                policy_name: "pivot_root",
                syscall_name: "pivot_root",
                syscall_number: 41,
            },
            SeccompSyscallProbe {
                policy_name: "chroot",
                syscall_name: "chroot",
                syscall_number: 51,
            },
            SeccompSyscallProbe {
                policy_name: "unshare",
                syscall_name: "unshare",
                syscall_number: 97,
            },
            SeccompSyscallProbe {
                policy_name: "setns",
                syscall_name: "setns",
                syscall_number: 268,
            },
            SeccompSyscallProbe {
                policy_name: "kexec_load",
                syscall_name: "kexec_load",
                syscall_number: 104,
            },
            SeccompSyscallProbe {
                policy_name: "kexec_file_load",
                syscall_name: "kexec_file_load",
                syscall_number: 294,
            },
            SeccompSyscallProbe {
                policy_name: "reboot",
                syscall_name: "reboot",
                syscall_number: 142,
            },
            SeccompSyscallProbe {
                policy_name: "init_module",
                syscall_name: "init_module",
                syscall_number: 105,
            },
            SeccompSyscallProbe {
                policy_name: "finit_module",
                syscall_name: "finit_module",
                syscall_number: 273,
            },
            SeccompSyscallProbe {
                policy_name: "delete_module",
                syscall_name: "delete_module",
                syscall_number: 106,
            },
            SeccompSyscallProbe {
                policy_name: "ptrace",
                syscall_name: "ptrace",
                syscall_number: 117,
            },
            SeccompSyscallProbe {
                policy_name: "process_vm_readv",
                syscall_name: "process_vm_readv",
                syscall_number: 270,
            },
            SeccompSyscallProbe {
                policy_name: "process_vm_writev",
                syscall_name: "process_vm_writev",
                syscall_number: 271,
            },
            SeccompSyscallProbe {
                policy_name: "bpf",
                syscall_name: "bpf",
                syscall_number: 280,
            },
            SeccompSyscallProbe {
                policy_name: "perf_event_open",
                syscall_name: "perf_event_open",
                syscall_number: 241,
            },
            SeccompSyscallProbe {
                policy_name: "userfaultfd",
                syscall_name: "userfaultfd",
                syscall_number: 282,
            },
            SeccompSyscallProbe {
                policy_name: "settimeofday",
                syscall_name: "settimeofday",
                syscall_number: 170,
            },
            SeccompSyscallProbe {
                policy_name: "adjtimex",
                syscall_name: "adjtimex",
                syscall_number: 171,
            },
            SeccompSyscallProbe {
                policy_name: "clock_settime",
                syscall_name: "clock_settime",
                syscall_number: 112,
            },
            SeccompSyscallProbe {
                policy_name: "sethostname",
                syscall_name: "sethostname",
                syscall_number: 161,
            },
            SeccompSyscallProbe {
                policy_name: "setdomainname",
                syscall_name: "setdomainname",
                syscall_number: 162,
            },
            SeccompSyscallProbe {
                policy_name: "swapon",
                syscall_name: "swapon",
                syscall_number: 224,
            },
            SeccompSyscallProbe {
                policy_name: "swapoff",
                syscall_name: "swapoff",
                syscall_number: 225,
            },
            SeccompSyscallProbe {
                policy_name: "syslog",
                syscall_name: "syslog",
                syscall_number: 116,
            },
            SeccompSyscallProbe {
                policy_name: "quotactl",
                syscall_name: "quotactl",
                syscall_number: 60,
            },
            SeccompSyscallProbe {
                policy_name: "add_key",
                syscall_name: "add_key",
                syscall_number: 217,
            },
            SeccompSyscallProbe {
                policy_name: "request_key",
                syscall_name: "request_key",
                syscall_number: 218,
            },
            SeccompSyscallProbe {
                policy_name: "keyctl",
                syscall_name: "keyctl",
                syscall_number: 219,
            },
        ]
    }

    #[cfg(target_arch = "x86_64")]
    fn default_seccomp_syscall_probes() -> Vec<SeccompSyscallProbe> {
        vec![
            SeccompSyscallProbe {
                policy_name: "mount",
                syscall_name: "mount",
                syscall_number: 165,
            },
            SeccompSyscallProbe {
                policy_name: "umount",
                syscall_name: "umount2",
                syscall_number: 166,
            },
            SeccompSyscallProbe {
                policy_name: "pivot_root",
                syscall_name: "pivot_root",
                syscall_number: 155,
            },
            SeccompSyscallProbe {
                policy_name: "chroot",
                syscall_name: "chroot",
                syscall_number: 161,
            },
            SeccompSyscallProbe {
                policy_name: "unshare",
                syscall_name: "unshare",
                syscall_number: 272,
            },
            SeccompSyscallProbe {
                policy_name: "setns",
                syscall_name: "setns",
                syscall_number: 308,
            },
            SeccompSyscallProbe {
                policy_name: "kexec_load",
                syscall_name: "kexec_load",
                syscall_number: 246,
            },
            SeccompSyscallProbe {
                policy_name: "kexec_file_load",
                syscall_name: "kexec_file_load",
                syscall_number: 320,
            },
            SeccompSyscallProbe {
                policy_name: "reboot",
                syscall_name: "reboot",
                syscall_number: 169,
            },
            SeccompSyscallProbe {
                policy_name: "init_module",
                syscall_name: "init_module",
                syscall_number: 175,
            },
            SeccompSyscallProbe {
                policy_name: "finit_module",
                syscall_name: "finit_module",
                syscall_number: 313,
            },
            SeccompSyscallProbe {
                policy_name: "delete_module",
                syscall_name: "delete_module",
                syscall_number: 176,
            },
            SeccompSyscallProbe {
                policy_name: "ptrace",
                syscall_name: "ptrace",
                syscall_number: 101,
            },
            SeccompSyscallProbe {
                policy_name: "process_vm_readv",
                syscall_name: "process_vm_readv",
                syscall_number: 310,
            },
            SeccompSyscallProbe {
                policy_name: "process_vm_writev",
                syscall_name: "process_vm_writev",
                syscall_number: 311,
            },
            SeccompSyscallProbe {
                policy_name: "bpf",
                syscall_name: "bpf",
                syscall_number: 321,
            },
            SeccompSyscallProbe {
                policy_name: "perf_event_open",
                syscall_name: "perf_event_open",
                syscall_number: 298,
            },
            SeccompSyscallProbe {
                policy_name: "userfaultfd",
                syscall_name: "userfaultfd",
                syscall_number: 323,
            },
            SeccompSyscallProbe {
                policy_name: "settimeofday",
                syscall_name: "settimeofday",
                syscall_number: 164,
            },
            SeccompSyscallProbe {
                policy_name: "adjtimex",
                syscall_name: "adjtimex",
                syscall_number: 159,
            },
            SeccompSyscallProbe {
                policy_name: "clock_settime",
                syscall_name: "clock_settime",
                syscall_number: 227,
            },
            SeccompSyscallProbe {
                policy_name: "sethostname",
                syscall_name: "sethostname",
                syscall_number: 170,
            },
            SeccompSyscallProbe {
                policy_name: "setdomainname",
                syscall_name: "setdomainname",
                syscall_number: 171,
            },
            SeccompSyscallProbe {
                policy_name: "swapon",
                syscall_name: "swapon",
                syscall_number: 167,
            },
            SeccompSyscallProbe {
                policy_name: "swapoff",
                syscall_name: "swapoff",
                syscall_number: 168,
            },
            SeccompSyscallProbe {
                policy_name: "syslog",
                syscall_name: "syslog",
                syscall_number: 103,
            },
            SeccompSyscallProbe {
                policy_name: "quotactl",
                syscall_name: "quotactl",
                syscall_number: 179,
            },
            SeccompSyscallProbe {
                policy_name: "add_key",
                syscall_name: "add_key",
                syscall_number: 248,
            },
            SeccompSyscallProbe {
                policy_name: "request_key",
                syscall_name: "request_key",
                syscall_number: 249,
            },
            SeccompSyscallProbe {
                policy_name: "keyctl",
                syscall_name: "keyctl",
                syscall_number: 250,
            },
            SeccompSyscallProbe {
                policy_name: "create_module",
                syscall_name: "create_module",
                syscall_number: 174,
            },
            SeccompSyscallProbe {
                policy_name: "get_kernel_syms",
                syscall_name: "get_kernel_syms",
                syscall_number: 177,
            },
            SeccompSyscallProbe {
                policy_name: "query_module",
                syscall_name: "query_module",
                syscall_number: 178,
            },
        ]
    }

    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    fn default_seccomp_syscall_probes() -> Vec<SeccompSyscallProbe> {
        Vec::new()
    }

    fn run_nsjail_python_args(
        custom_image: Option<&str>,
        args: Vec<String>,
        timeout_secs: u32,
    ) -> Option<SandboxResult<SandboxOutput>> {
        if !nsjail_available() {
            eprintln!("nsjail not found, skipping test");
            return None;
        }

        use super::super::{SandboxContext, SandboxResources, ScriptLang};
        use std::collections::HashMap;

        let tmp = match tempfile::tempdir() {
            Ok(tmp) => tmp,
            Err(err) => panic!("failed to create temp dir: {err}"),
        };
        let run_dir = tmp.path().to_string_lossy().to_string();

        let mut config = NsjailConfig::default();
        config.cgroup_pids_max = 0;
        let sandbox = NsjailSandbox::new(config);

        let ctx = SandboxContext {
            run_id: uuid::Uuid::new_v4(),
            run_dir,
            command: "python".to_string(),
            args,
            env: HashMap::new(),
            timeout_secs,
            language: ScriptLang::Python3,
            custom_image: custom_image.map(String::from),
            trace_context: None,
            resource_limits: SandboxResources {
                cpu: 0.0,
                memory_bytes: 0,
                disk_bytes: 0,
                timeout_secs,
            },
            stdin: None,
            allow_network: false,
        };

        let rt = match tokio::runtime::Runtime::new() {
            Ok(rt) => rt,
            Err(err) => panic!("failed to create Tokio runtime: {err}"),
        };
        let cancel = tokio_util::sync::CancellationToken::new();
        Some(rt.block_on(sandbox.execute(&ctx, cancel)))
    }

    #[test]
    fn test_resolve_runtime_default() {
        let config = NsjailConfig::default();
        let sandbox = NsjailSandbox::new(config);
        let entry = runtime_entry(sandbox.resolve_runtime(None));
        assert_eq!(entry.rootfs, "/opt/sandbox-rootfs/python-3.12");
        assert_eq!(entry.command, "/usr/local/bin/python");
    }

    #[test]
    fn test_resolve_runtime_specific_version() {
        let config = NsjailConfig::default();
        let sandbox = NsjailSandbox::new(config);
        let entry = runtime_entry(sandbox.resolve_runtime(Some("python:3.11")));
        assert_eq!(entry.rootfs, "/opt/sandbox-rootfs/python-3.11");
    }

    #[test]
    fn test_resolve_runtime_unknown_is_rejected() {
        let config = NsjailConfig::default();
        let sandbox = NsjailSandbox::new(config);
        let err = sandbox
            .resolve_runtime(Some("python:3.99"))
            .expect_err("unknown runtime should error, not fall back");
        match err {
            SandboxError::Other(msg) => assert!(
                msg.contains("python:3.99"),
                "error should mention requested runtime: {msg}"
            ),
            other => panic!("unexpected error variant: {other:?}"),
        }
    }

    #[test]
    fn test_build_nsjail_args_basic() {
        use super::super::{SandboxContext, SandboxResources, ScriptLang};
        use std::collections::HashMap;

        let config = NsjailConfig::default();
        let sandbox = NsjailSandbox::new(config);

        let ctx = SandboxContext {
            run_id: uuid::Uuid::nil(),
            run_dir: "/tmp/run-123".to_string(),
            command: "python".to_string(), // ignored for nsjail, uses catalog
            args: vec!["-c".to_string(), "print('hello')".to_string()],
            env: HashMap::from([("CF_RUN_ID".to_string(), "abc".to_string())]),
            timeout_secs: 30,
            language: ScriptLang::Python3,
            custom_image: None,
            trace_context: None,
            resource_limits: SandboxResources {
                cpu: 1.0,
                memory_bytes: 512 * 1024 * 1024,
                disk_bytes: 1024 * 1024 * 1024,
                timeout_secs: 30,
            },
            stdin: None,
            allow_network: false,
        };

        let args = nsjail_args(sandbox.build_nsjail_args(&ctx));

        assert!(args.contains(&"--mode".to_string()));
        assert!(args.contains(&"o".to_string()));
        assert!(args.contains(&"--chroot".to_string()));
        assert!(args.contains(&"/opt/sandbox-rootfs/python-3.12".to_string()));
        assert!(args.contains(&"--user".to_string()));
        assert!(args.contains(&"99999".to_string()));
        assert!(args.contains(&"--time_limit".to_string()));
        assert!(args.contains(&"30".to_string()));
        assert!(args.contains(&"--cgroup_mem_swap_max".to_string()));
        assert!(args.contains(&"--cgroup_cpu_ms_per_sec".to_string()));
        assert!(args.contains(&"1000".to_string())); // 1.0 cpu = 1000ms/sec
        // Default config has clone_newnet=true and ctx.allow_network=false →
        // a new netns is created and loopback is disabled inside it.
        assert!(!args.contains(&"--disable_clone_newnet".to_string()));
        assert!(args.contains(&"--iface_no_lo".to_string()));
        // Default config ships a non-empty seccomp policy.
        assert!(args.contains(&"--seccomp_string".to_string()));
        assert!(args.contains(&"--cwd".to_string()));
        assert!(args.contains(&"/work".to_string()));
        assert!(args.contains(&"--bindmount".to_string()));
        assert!(args.contains(&"/tmp/run-123:/work".to_string()));
        assert!(args.contains(&"--env".to_string()));
        assert!(args.contains(&"CF_RUN_ID=abc".to_string()));
        assert!(args.contains(&"--".to_string()));
        assert!(args.contains(&"/usr/local/bin/python".to_string()));
        assert!(args.contains(&"-c".to_string()));
        assert!(args.contains(&"print('hello')".to_string()));
    }

    #[test]
    fn test_build_nsjail_args_allow_network_shares_host_netns() {
        use super::super::{SandboxContext, SandboxResources, ScriptLang};
        use std::collections::HashMap;

        let config = NsjailConfig::default();
        let sandbox = NsjailSandbox::new(config);

        let ctx = SandboxContext {
            run_id: uuid::Uuid::nil(),
            run_dir: "/tmp/run-net".to_string(),
            command: "python".to_string(),
            args: vec!["-c".to_string(), "import urllib".to_string()],
            env: HashMap::new(),
            timeout_secs: 30,
            language: ScriptLang::Python3,
            custom_image: None,
            trace_context: None,
            resource_limits: SandboxResources {
                cpu: 1.0,
                memory_bytes: 256 * 1024 * 1024,
                disk_bytes: 256 * 1024 * 1024,
                timeout_secs: 30,
            },
            stdin: None,
            allow_network: true,
        };

        let args = nsjail_args(sandbox.build_nsjail_args(&ctx));

        // allow_network=true → share host netns (pip install path).
        assert!(args.contains(&"--disable_clone_newnet".to_string()));
        // Loopback flag should NOT be present: it only takes effect inside a
        // new netns, so emitting it under host-share would be misleading.
        assert!(!args.contains(&"--iface_no_lo".to_string()));
    }

    #[test]
    fn test_build_nsjail_args_clone_newnet_disabled_in_config() {
        use super::super::{SandboxContext, SandboxResources, ScriptLang};
        use std::collections::HashMap;

        let mut config = NsjailConfig::default();
        config.clone_newnet = false; // operator override: host netns for all
        let sandbox = NsjailSandbox::new(config);

        let ctx = SandboxContext {
            run_id: uuid::Uuid::nil(),
            run_dir: "/tmp/run".to_string(),
            command: "python".to_string(),
            args: vec![],
            env: HashMap::new(),
            timeout_secs: 10,
            language: ScriptLang::Python3,
            custom_image: None,
            trace_context: None,
            resource_limits: SandboxResources {
                cpu: 1.0,
                memory_bytes: 0,
                disk_bytes: 0,
                timeout_secs: 10,
            },
            stdin: None,
            allow_network: false,
        };

        let args = nsjail_args(sandbox.build_nsjail_args(&ctx));
        // Even with allow_network=false, an operator who disables clone_newnet
        // in config opts the whole worker out of net isolation.
        assert!(args.contains(&"--disable_clone_newnet".to_string()));
    }

    #[test]
    fn test_build_nsjail_args_custom_image() {
        use super::super::{SandboxContext, SandboxResources, ScriptLang};
        use std::collections::HashMap;

        let config = NsjailConfig::default();
        let sandbox = NsjailSandbox::new(config);

        let ctx = SandboxContext {
            run_id: uuid::Uuid::nil(),
            run_dir: "/tmp/run-456".to_string(),
            command: "python".to_string(),
            args: vec![
                "-c".to_string(),
                "import sys; print(sys.version)".to_string(),
            ],
            env: HashMap::new(),
            timeout_secs: 60,
            language: ScriptLang::Python3,
            custom_image: Some("python:3.11".to_string()),
            trace_context: None,
            resource_limits: SandboxResources {
                cpu: 1.0,
                memory_bytes: 256 * 1024 * 1024,
                disk_bytes: 512 * 1024 * 1024,
                timeout_secs: 60,
            },
            stdin: None,
            allow_network: false,
        };

        let args = nsjail_args(sandbox.build_nsjail_args(&ctx));

        // Should use python:3.11 rootfs
        assert!(args.contains(&"/opt/sandbox-rootfs/python-3.11".to_string()));
    }

    #[test]
    fn test_name() {
        let config = NsjailConfig::default();
        let sandbox = NsjailSandbox::new(config);
        assert_eq!(sandbox.name(), "nsjail");
    }

    #[test]
    fn test_supports_resource_limits() {
        let config = NsjailConfig::default();
        let sandbox = NsjailSandbox::new(config);
        assert!(sandbox.supports_resource_limits());
    }

    // -- Integration tests (require nsjail binary) --

    #[test]
    fn test_health_check_nsjail() {
        require_nsjail!();

        let config = NsjailConfig::default();
        let sandbox = NsjailSandbox::new(config);
        let rt = tokio::runtime::Runtime::new().unwrap();
        // health_check verifies the binary and rootfs exist
        let result = rt.block_on(sandbox.health_check());
        assert!(result.is_ok(), "health_check failed: {:?}", result.err());
    }

    /// Helper: run a simple Python script inside nsjail with the given runtime.
    /// `custom_image` is e.g. `Some("python:3.11")` or `None` for the default.
    fn run_nsjail_python(custom_image: Option<&str>) {
        require_nsjail!();

        use super::super::{SandboxContext, SandboxResources, ScriptLang};
        use std::collections::HashMap;

        let tmp = tempfile::tempdir().unwrap();
        let run_dir = tmp.path().to_string_lossy().to_string();

        // Disable cgroup-based limits for basic execution tests.
        // Cgroup limits are tested separately in test_cgroup_* tests.
        let mut config = NsjailConfig::default();
        config.cgroup_pids_max = 0;
        let sandbox = NsjailSandbox::new(config);

        let ctx = SandboxContext {
            run_id: uuid::Uuid::new_v4(),
            run_dir,
            command: "python".to_string(),
            args: vec![
                "-c".to_string(),
                "import sys; print(f'hello from python {sys.version_info.major}.{sys.version_info.minor}')".to_string(),
            ],
            env: HashMap::new(),
            timeout_secs: 10,
            language: ScriptLang::Python3,
            custom_image: custom_image.map(String::from),
            trace_context: None,
            resource_limits: SandboxResources {
                cpu: 0.0,
                memory_bytes: 0,
                disk_bytes: 0,
                timeout_secs: 10,
            },
            stdin: None,
            allow_network: false,
        };

        let rt = tokio::runtime::Runtime::new().unwrap();
        let cancel = tokio_util::sync::CancellationToken::new();
        let result = rt.block_on(sandbox.execute(&ctx, cancel));

        assert!(result.is_ok(), "execute failed: {:?}", result.err());
        let output = result.unwrap();
        assert_eq!(
            output.exit_code, 0,
            "nsjail stdout:\n{}\nnsjail stderr:\n{}",
            output.stdout, output.stderr
        );
        assert!(
            output.stdout.contains("hello from python"),
            "unexpected stdout: {}",
            output.stdout
        );
    }

    #[test]
    fn test_execute_simple_script() {
        run_nsjail_python(None);
    }

    #[test]
    fn test_execute_python311() {
        run_nsjail_python(Some("python:3.11"));
    }

    #[test]
    fn test_execute_python312() {
        run_nsjail_python(Some("python:3.12"));
    }

    #[test]
    #[ignore = "requires nsjail, Python rootfs, and seccomp support"]
    fn test_default_seccomp_policy_blocks_dangerous_syscalls() {
        require_nsjail!();

        let probes = default_seccomp_syscall_probes();
        if probes.is_empty() {
            eprintln!("no syscall probes for this architecture, skipping test");
            return;
        }

        let denied = default_seccomp_denied_syscalls();
        for syscall in &denied {
            assert!(
                probes.iter().any(|probe| probe.policy_name == *syscall),
                "default seccomp policy lacks a runtime probe for syscall '{syscall}'"
            );
        }

        for runtime_name in default_runtime_names() {
            for probe in &probes {
                let script = format!(
                    r#"
import ctypes
import sys

sys.stdout.write("about_to_call:{policy_name}:{syscall_name}:{syscall_number}\n")
sys.stdout.flush()
libc = ctypes.CDLL(None, use_errno=True)
rc = libc.syscall({syscall_number}, 0, 0, 0, 0, 0, 0)
err = ctypes.get_errno()
print(f"syscall_returned:rc={{rc}}:errno={{err}}", flush=True)
raise SystemExit(42)
"#,
                    policy_name = probe.policy_name,
                    syscall_name = probe.syscall_name,
                    syscall_number = probe.syscall_number,
                );

                let result = run_nsjail_python_args(
                    Some(runtime_name.as_str()),
                    vec!["-c".to_string(), script],
                    10,
                )
                .unwrap_or_else(|| {
                    panic!(
                        "nsjail unavailable while probing runtime '{}' seccomp syscall '{}'",
                        runtime_name, probe.policy_name
                    )
                });
                let output = match result {
                    Ok(output) => output,
                    Err(err) => panic!(
                        "nsjail execute failed for runtime '{}' syscall '{}': {err:?}",
                        runtime_name, probe.policy_name
                    ),
                };

                assert_ne!(
                    output.exit_code,
                    0,
                    "default seccomp policy did not block '{}' on runtime '{}'/{}; stdout:\n{}\nstderr:\n{}",
                    probe.policy_name,
                    runtime_name,
                    probe.syscall_name,
                    output.stdout,
                    output.stderr
                );
                assert!(
                    output.stdout.contains("about_to_call"),
                    "probe for '{}' on runtime '{}/{}' did not reach the syscall call site; stdout:\n{}\nstderr:\n{}",
                    probe.policy_name,
                    runtime_name,
                    probe.syscall_name,
                    output.stdout,
                    output.stderr
                );
                assert!(
                    !output.stdout.contains("syscall_returned"),
                    "default seccomp policy allowed '{}' on runtime '{}'/{} to return; stdout:\n{}\nstderr:\n{}",
                    probe.policy_name,
                    runtime_name,
                    probe.syscall_name,
                    output.stdout,
                    output.stderr
                );
            }
        }
    }

    // -- Cgroup integration tests (require nsjail + cgroupv2 on native Linux) --

    /// Check if cgroupv2 is available and nsjail can create sub-cgroups.
    /// This works on native Linux (e.g. GitHub Actions runners) where
    /// cgroupv2 is available and writable.
    fn cgroup_v2_available() -> bool {
        std::path::Path::new("/sys/fs/cgroup/cgroup.controllers").exists()
    }

    macro_rules! require_cgroup {
        () => {
            require_nsjail!();
            if !cgroup_v2_available() {
                eprintln!("cgroupv2 not available, skipping test");
                return;
            }
        };
    }

    #[test]
    fn test_cgroup_memory_limit_oom() {
        require_cgroup!();

        use super::super::{SandboxContext, SandboxResources, ScriptLang};
        use std::collections::HashMap;

        let tmp = tempfile::tempdir().unwrap();
        let run_dir = tmp.path().to_string_lossy().to_string();

        let mut config = NsjailConfig::default();
        config.cgroup_pids_max = 0;
        let sandbox = NsjailSandbox::new(config);

        // 50MB memory limit; Python needs ~30MB to start, then the script
        // tries to allocate 200MB which pushes it well over the limit.
        let ctx = SandboxContext {
            run_id: uuid::Uuid::new_v4(),
            run_dir,
            command: "python".to_string(),
            args: vec![
                "-c".to_string(),
                "x = bytearray(200 * 1024 * 1024); print('allocated')".to_string(),
            ],
            env: HashMap::new(),
            timeout_secs: 10,
            language: ScriptLang::Python3,
            custom_image: None,
            trace_context: None,
            resource_limits: SandboxResources {
                cpu: 0.0,
                memory_bytes: 50 * 1024 * 1024,
                disk_bytes: 0,
                timeout_secs: 10,
            },
            stdin: None,
            allow_network: false,
        };

        let rt = tokio::runtime::Runtime::new().unwrap();
        let cancel = tokio_util::sync::CancellationToken::new();
        let result = rt.block_on(sandbox.execute(&ctx, cancel));

        match result {
            Ok(output) => {
                assert_ne!(
                    output.exit_code, 0,
                    "expected non-zero exit due to OOM, got stdout: {}",
                    output.stdout
                );
                assert!(
                    !output.stdout.contains("allocated"),
                    "process should have been killed before printing"
                );
            }
            Err(_) => {
                // nsjail itself may error out on cgroup init failure
            }
        }
    }

    #[test]
    fn test_cgroup_memory_within_bounds() {
        require_cgroup!();

        use super::super::{SandboxContext, SandboxResources, ScriptLang};
        use std::collections::HashMap;

        let tmp = tempfile::tempdir().unwrap();
        let run_dir = tmp.path().to_string_lossy().to_string();

        let mut config = NsjailConfig::default();
        config.cgroup_pids_max = 0;
        let sandbox = NsjailSandbox::new(config);

        // 128MB limit; script allocates only 1MB.
        let ctx = SandboxContext {
            run_id: uuid::Uuid::new_v4(),
            run_dir,
            command: "python".to_string(),
            args: vec![
                "-c".to_string(),
                "x = bytearray(1 * 1024 * 1024); print('ok')".to_string(),
            ],
            env: HashMap::new(),
            timeout_secs: 10,
            language: ScriptLang::Python3,
            custom_image: None,
            trace_context: None,
            resource_limits: SandboxResources {
                cpu: 0.0,
                memory_bytes: 128 * 1024 * 1024,
                disk_bytes: 0,
                timeout_secs: 10,
            },
            stdin: None,
            allow_network: false,
        };

        let rt = tokio::runtime::Runtime::new().unwrap();
        let cancel = tokio_util::sync::CancellationToken::new();
        let result = rt.block_on(sandbox.execute(&ctx, cancel));

        assert!(result.is_ok(), "execute failed: {:?}", result.err());
        let output = result.unwrap();
        assert_eq!(output.exit_code, 0, "stderr: {}", output.stderr);
        assert!(
            output.stdout.contains("ok"),
            "unexpected stdout: {}",
            output.stdout
        );
    }

    #[test]
    fn test_cgroup_cpu_limit() {
        require_cgroup!();

        use super::super::{SandboxContext, SandboxResources, ScriptLang};
        use std::collections::HashMap;

        let tmp = tempfile::tempdir().unwrap();
        let run_dir = tmp.path().to_string_lossy().to_string();

        let mut config = NsjailConfig::default();
        config.cgroup_pids_max = 0;
        let sandbox = NsjailSandbox::new(config);

        // Restrict to 0.5 CPU (500ms/sec). Run a busy loop and verify
        // the process completes (cgroup cpu throttling, not a hard kill).
        let ctx = SandboxContext {
            run_id: uuid::Uuid::new_v4(),
            run_dir,
            command: "python".to_string(),
            args: vec![
                "-c".to_string(),
                "s = sum(range(500000)); print(f'sum={s}')".to_string(),
            ],
            env: HashMap::new(),
            timeout_secs: 30,
            language: ScriptLang::Python3,
            custom_image: None,
            trace_context: None,
            resource_limits: SandboxResources {
                cpu: 0.5,
                memory_bytes: 0,
                disk_bytes: 0,
                timeout_secs: 30,
            },
            stdin: None,
            allow_network: false,
        };

        let rt = tokio::runtime::Runtime::new().unwrap();
        let cancel = tokio_util::sync::CancellationToken::new();
        let result = rt.block_on(sandbox.execute(&ctx, cancel));

        assert!(result.is_ok(), "execute failed: {:?}", result.err());
        let output = result.unwrap();
        assert_eq!(output.exit_code, 0, "stderr: {}", output.stderr);
        assert!(
            output.stdout.contains("sum="),
            "unexpected stdout: {}",
            output.stdout
        );
    }

    #[test]
    fn test_cgroup_pids_limit() {
        require_cgroup!();

        use super::super::{SandboxContext, SandboxResources, ScriptLang};
        use std::collections::HashMap;

        let tmp = tempfile::tempdir().unwrap();
        let run_dir = tmp.path().to_string_lossy().to_string();

        let mut config = NsjailConfig::default();
        config.cgroup_pids_max = 5;
        let sandbox = NsjailSandbox::new(config);

        // Try to fork 20 child processes with a pids limit of 5.
        let ctx = SandboxContext {
            run_id: uuid::Uuid::new_v4(),
            run_dir,
            command: "python".to_string(),
            args: vec![
                "-c".to_string(),
                concat!(
                    "import os, sys\n",
                    "children = []\n",
                    "for i in range(20):\n",
                    "    try:\n",
                    "        pid = os.fork()\n",
                    "        if pid == 0:\n",
                    "            os._exit(0)\n",
                    "        children.append(pid)\n",
                    "    except OSError:\n",
                    "        print(f'fork failed at {i}')\n",
                    "        break\n",
                    "for p in children:\n",
                    "    os.waitpid(p, 0)\n",
                    "print(f'forked={len(children)}')\n",
                )
                .to_string(),
            ],
            env: HashMap::new(),
            timeout_secs: 10,
            language: ScriptLang::Python3,
            custom_image: None,
            trace_context: None,
            resource_limits: SandboxResources {
                cpu: 0.0,
                memory_bytes: 0,
                disk_bytes: 0,
                timeout_secs: 10,
            },
            stdin: None,
            allow_network: false,
        };

        let rt = tokio::runtime::Runtime::new().unwrap();
        let cancel = tokio_util::sync::CancellationToken::new();
        let result = rt.block_on(sandbox.execute(&ctx, cancel));

        match result {
            Ok(output) => {
                // With pids limit of 5, fork should fail well before 20
                if output.stdout.contains("forked=") {
                    let forked: usize = output
                        .stdout
                        .lines()
                        .find_map(|l| l.strip_prefix("forked="))
                        .and_then(|n| n.trim().parse().ok())
                        .unwrap_or(0);
                    assert!(
                        forked < 20,
                        "expected pids limit to restrict forking, but forked {forked}"
                    );
                }
                // Non-zero exit is also acceptable (killed by cgroup)
            }
            Err(_) => {
                // nsjail may error out on cgroup init
            }
        }
    }
}
