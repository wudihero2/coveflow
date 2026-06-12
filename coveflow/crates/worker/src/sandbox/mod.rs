pub(crate) mod none;
pub(crate) mod signals;

#[cfg(target_os = "linux")]
pub(crate) mod nsjail;

use async_trait::async_trait;
use process_wrap::tokio::*;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_util::sync::CancellationToken;

use crate::config::{K8sPodConfig, NsjailConfig};
use crate::error::{SandboxError, SandboxResult};
use crate::sandbox::none::NoneSandbox;
use crate::sandbox::signals::{kill_process_tree, signal_chain};

#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(tag = "mode")]
pub enum SandboxMode {
    #[default]
    None,
    Nsjail(NsjailConfig),
    KubernetesPod(K8sPodConfig),
}

impl SandboxMode {
    pub fn name(&self) -> &str {
        match self {
            SandboxMode::None => "none",
            SandboxMode::Nsjail(_) => "nsjail",
            SandboxMode::KubernetesPod(_) => "k8s",
        }
    }

    pub(crate) fn supports_disk_limit(&self) -> bool {
        match self {
            SandboxMode::Nsjail(_) => true,
            SandboxMode::None | SandboxMode::KubernetesPod(_) => false,
        }
    }
}

#[async_trait]
pub trait Sandbox: Send + Sync {
    async fn execute(
        &self,
        ctx: &SandboxContext,
        cancel: tokio_util::sync::CancellationToken,
    ) -> SandboxResult<SandboxOutput>;

    async fn health_check(&self) -> SandboxResult<()>;

    fn name(&self) -> &str;

    fn supports_resource_limits(&self) -> bool {
        true
    }
}

pub struct SandboxContext {
    pub run_id: uuid::Uuid,
    pub run_dir: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: std::collections::HashMap<String, String>,
    pub timeout_secs: u32,
    pub language: ScriptLang,
    pub custom_image: Option<String>,
    pub trace_context: Option<TraceContext>,
    pub resource_limits: SandboxResources,
    pub stdin: Option<Vec<u8>>,
    /// Whether to allow access to the host network namespace.
    /// Set true only for trusted internal stages (e.g. pip install).
    /// User script execution must keep this false to prevent SSRF,
    /// cloud-metadata access, and host-service exfiltration.
    pub allow_network: bool,
}

pub use coveflow_types::scripts::ScriptLang;

pub struct TraceContext {
    pub trace_id: String,
    pub span_id: String,
}

pub struct SandboxResources {
    pub cpu: f32,
    pub memory_bytes: u64,
    pub disk_bytes: u64,
    pub timeout_secs: u32,
}

impl Default for SandboxResources {
    fn default() -> Self {
        Self {
            cpu: 1.0,
            memory_bytes: 512 * 1024 * 1024,
            disk_bytes: 1024 * 1024 * 1024,
            timeout_secs: 3600,
        }
    }
}

pub struct SandboxOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
    pub memory_peak_bytes: u64,
}

pub struct SandboxRouter {
    none: NoneSandbox,
    #[cfg(target_os = "linux")]
    nsjail: Option<nsjail::NsjailSandbox>,
}

impl SandboxRouter {
    pub fn new(mode: &SandboxMode) -> Self {
        #[cfg(target_os = "linux")]
        let nsjail = match mode {
            SandboxMode::Nsjail(config) => Some(nsjail::NsjailSandbox::new(config.clone())),
            _ => None,
        };

        #[cfg(not(target_os = "linux"))]
        let _ = mode;

        Self {
            none: NoneSandbox,
            #[cfg(target_os = "linux")]
            nsjail,
        }
    }

    pub fn select(&self, tag: &str) -> &dyn Sandbox {
        match tag {
            "none" | "dev" => &self.none,
            _ => self.select_default(),
        }
    }

    fn select_default(&self) -> &dyn Sandbox {
        #[cfg(target_os = "linux")]
        if let Some(s) = &self.nsjail {
            return s;
        }
        &self.none
    }
}

/// Hard cap on captured stdout/stderr bytes per stream. Beyond this we keep
/// draining the pipe so the child does not block, but stop appending to memory.
/// Sized to fit comfortably within the 10MB result limit plus headroom for stderr.
pub(crate) const MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sandbox_mode_disk_limit_capability_is_explicit() {
        assert!(!SandboxMode::None.supports_disk_limit());
        assert!(SandboxMode::Nsjail(NsjailConfig::default()).supports_disk_limit());

        let k8s = SandboxMode::KubernetesPod(K8sPodConfig {
            namespace: "default".to_string(),
            default_image: "python:3.12".to_string(),
            request_ratio: 1.0,
            service_account: None,
            node_selector: None,
            image_pull_secrets: Vec::new(),
            auto_cleanup: true,
        });
        assert!(!k8s.supports_disk_limit());
    }
}

macro_rules! spawn_line_reader {
    ($stream:expr, $target:literal, $level:ident) => {
        tokio::spawn(async move {
            let mut lines = Vec::new();
            let mut total_bytes: usize = 0;
            let mut truncated = false;
            if let Some(inner) = $stream {
                let reader = BufReader::new(inner);
                let mut line_stream = reader.lines();
                while let Ok(Some(line)) = line_stream.next_line().await {
                    if !truncated {
                        // +1 accounts for the newline that line_stream stripped.
                        total_bytes = total_bytes.saturating_add(line.len() + 1);
                        if total_bytes <= MAX_OUTPUT_BYTES {
                            tracing::$level!(target: $target, "{}", line);
                            lines.push(line);
                        } else {
                            truncated = true;
                            let marker = format!(
                                "[output truncated at {} bytes]",
                                MAX_OUTPUT_BYTES
                            );
                            tracing::warn!(
                                target: $target,
                                limit = MAX_OUTPUT_BYTES,
                                "captured output exceeded limit, truncating"
                            );
                            lines.push(marker);
                        }
                    }
                    // After truncation: keep draining the pipe so the child
                    // does not block on a full stdout/stderr buffer, but drop
                    // the bytes to avoid OOMing the worker.
                }
            }
            lines
        })
    };
}

/// Shared execution logic for sandbox implementations.
///
/// Each sandbox builds a `Command` and delegates to this helper which handles:
/// process group isolation, stdin piping, spawning, timeout/cancel via `tokio::select!`,
/// and output construction.
///
/// The child process is spawned as its own process group leader (via `process-wrap`),
/// ensuring `kill(-pid)` targets only the child tree and not the worker.
///
/// `check_exit` is an optional callback invoked when the process exits normally.
/// It receives the exit code and the context's `timeout_secs`.
/// If it returns `Some(err)`, that error is returned instead of the normal output.
#[tracing::instrument(name = "sandbox::run_child", skip(cmd, ctx, cancel, check_exit), fields(%label, timeout_secs = timeout.as_secs()))]
pub(crate) async fn run_child(
    mut cmd: tokio::process::Command,
    ctx: &SandboxContext,
    cancel: CancellationToken,
    timeout: std::time::Duration,
    label: &str,
    check_exit: Option<fn(i32, u32) -> Option<SandboxError>>,
) -> SandboxResult<SandboxOutput> {
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    if ctx.stdin.is_some() {
        cmd.stdin(std::process::Stdio::piped());
    }

    // Wrap command to spawn child as its own process group leader.
    // This ensures kill_process_tree(-pid) only targets the child and its descendants.
    let mut cmd_wrap = TokioCommandWrap::from(cmd);
    cmd_wrap.wrap(ProcessGroup::leader());

    let start = std::time::Instant::now();
    let mut child = cmd_wrap.spawn()?;
    let pid = child.id().ok_or_else(|| {
        SandboxError::Other(format!("{label} child process has no PID (already exited)"))
    })?;
    tracing::info!(
        label,
        pid,
        "sandbox process spawned (cancellable, pgid=pid)"
    );

    // Write stdin if needed, then drop to signal EOF
    if let Some(data) = &ctx.stdin {
        if let Some(mut child_stdin) = child.stdin().take() {
            child_stdin.write_all(data).await?;
            drop(child_stdin);
        }
    }

    // Take stdout/stderr handles for streaming reads.
    // Each line is emitted via tracing for DbLogLayer to capture in real-time.
    let child_stdout = child.stdout().take();
    let child_stderr = child.stderr().take();

    let stdout_task = spawn_line_reader!(child_stdout, "run_stdout", info);
    let stderr_task = spawn_line_reader!(child_stderr, "run_stderr", warn);

    tokio::select! {
        status = Box::into_pin(child.wait()) => {
            let duration_ms = start.elapsed().as_millis() as u64;
            match status {
                Ok(exit_status) => {
                    let exit_code = exit_status.code().unwrap_or(-1);

                    // Wait for reader tasks to finish collecting output
                    let stdout_lines = stdout_task.await.unwrap_or_default();
                    let stderr_lines = stderr_task.await.unwrap_or_default();

                    if let Some(check) = check_exit {
                        if let Some(err) = check(exit_code, ctx.timeout_secs) {
                            return Err(err);
                        }
                    }

                    tracing::info!(label, exit_code, duration_ms, "sandbox execution completed");
                    Ok(SandboxOutput {
                        exit_code,
                        stdout: stdout_lines.join("\n"),
                        stderr: stderr_lines.join("\n"),
                        duration_ms,
                        memory_peak_bytes: 0,
                    })
                }
                Err(e) => {
                    stdout_task.abort();
                    stderr_task.abort();
                    tracing::error!(label, error = %e, "sandbox process IO error");
                    Err(SandboxError::Io(e))
                }
            }
        }

        () = tokio::time::sleep(timeout) => {
            tracing::warn!(label, timeout_secs = ctx.timeout_secs, pid, "sandbox process timed out, killing");
            stdout_task.abort();
            stderr_task.abort();
            kill_process_tree(pid).await;
            Err(SandboxError::Timeout(ctx.timeout_secs))
        }

        () = cancel.cancelled() => {
            tracing::info!(label, pid, "cancel signal received, initiating signal chain");
            stdout_task.abort();
            stderr_task.abort();
            signal_chain(pid).await;
            Err(SandboxError::Canceled("run was canceled".to_string()))
        }
    }
}
