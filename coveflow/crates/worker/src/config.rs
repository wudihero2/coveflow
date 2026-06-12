use std::collections::HashMap;

#[derive(Debug, Clone, serde::Deserialize)]
pub struct RuntimeEntry {
    pub rootfs: String,
    pub command: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct RuntimeCatalog {
    pub default_runtime: String,
    pub runtimes: HashMap<String, RuntimeEntry>,
}

impl Default for RuntimeCatalog {
    fn default() -> Self {
        let versions = ["3.11", "3.12"];
        let mut runtimes = HashMap::new();
        for v in versions {
            runtimes.insert(
                format!("python:{v}"),
                RuntimeEntry {
                    rootfs: format!("/opt/sandbox-rootfs/python-{v}"),
                    command: "/usr/local/bin/python".to_string(),
                },
            );
        }
        Self {
            default_runtime: "python:3.12".to_string(),
            runtimes,
        }
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct NsjailConfig {
    pub nsjail_path: String,
    /// Whether to isolate the sandbox into its own network namespace.
    /// Defaults to true (no network). When false, the sandbox shares the
    /// host netns and can reach 127.0.0.1, cloud metadata (169.254.169.254),
    /// and any host-reachable internal services — only set false for trusted
    /// stages (e.g. pip install) via per-context `allow_network`.
    pub clone_newnet: bool,
    pub work_mount: String,
    pub uid: u32,
    pub gid: u32,
    pub cgroup_pids_max: u32,
    pub extra_bind_mounts_ro: Vec<String>,
    /// Optional Kafel seccomp policy passed via nsjail `--seccomp_string`.
    /// Default denies a small set of high-risk syscalls (mount/ptrace/bpf/...).
    /// Empty string disables seccomp.
    #[serde(default = "default_seccomp_policy")]
    pub seccomp_policy: String,
    #[serde(default)]
    pub catalog: RuntimeCatalog,
}

/// Minimal Kafel deny policy for Python workloads.
/// Denylist style: ALLOW by default; KILL_PROCESS on a curated set of high-risk
/// syscalls. Chosen for compatibility with normal Python + pip native extensions.
/// Blocks: namespace/mount escapes, ptrace, kernel module loading, BPF, host
/// timekeeping/hostname, swap/reboot, keyring, perf_event_open.
pub fn default_seccomp_policy() -> String {
    let denied = default_seccomp_denied_syscalls();

    format!(
        "POLICY coveflow_default {{\n  KILL_PROCESS {{\n    {}\n  }}\n}}\nUSE coveflow_default DEFAULT ALLOW",
        denied.join(",\n    ")
    )
}

pub(crate) fn default_seccomp_denied_syscalls() -> Vec<&'static str> {
    let mut syscalls = vec![
        "mount",
        "umount",
        "pivot_root",
        "chroot",
        "unshare",
        "setns",
        "kexec_load",
        "kexec_file_load",
        "reboot",
        "init_module",
        "finit_module",
        "delete_module",
        "ptrace",
        "process_vm_readv",
        "process_vm_writev",
        "bpf",
        "perf_event_open",
        "userfaultfd",
        "settimeofday",
        "adjtimex",
        "clock_settime",
        "sethostname",
        "setdomainname",
        "swapon",
        "swapoff",
        "syslog",
        "quotactl",
        "add_key",
        "request_key",
        "keyctl",
    ];

    syscalls.extend_from_slice(arch_specific_seccomp_denied_syscalls());
    syscalls
}

#[cfg(target_arch = "x86_64")]
fn arch_specific_seccomp_denied_syscalls() -> &'static [&'static str] {
    &["create_module", "get_kernel_syms", "query_module"]
}

#[cfg(not(target_arch = "x86_64"))]
fn arch_specific_seccomp_denied_syscalls() -> &'static [&'static str] {
    &[]
}

impl Default for NsjailConfig {
    fn default() -> Self {
        Self {
            nsjail_path: "/usr/sbin/nsjail".to_string(),
            clone_newnet: true,
            work_mount: "/work".to_string(),
            uid: 99999,
            gid: 99999,
            cgroup_pids_max: 64,
            extra_bind_mounts_ro: Vec::new(),
            seccomp_policy: default_seccomp_policy(),
            catalog: RuntimeCatalog::default(),
        }
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct K8sPodConfig {
    pub namespace: String,
    pub default_image: String,
    pub request_ratio: f32,
    pub service_account: Option<String>,
    pub node_selector: Option<HashMap<String, String>>,
    pub image_pull_secrets: Vec<String>,
    pub auto_cleanup: bool,
}
