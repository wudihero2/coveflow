use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;

/// Gives the process a chance to clean up gracefully.
/// Sends SIGINT → (3s) → SIGTERM → (5s) → SIGKILL.
#[tracing::instrument(name = "sandbox::signal_chain", fields(pid))]
pub(crate) async fn signal_chain(pid: u32) {
    let nix_pid = Pid::from_raw(pid as i32);

    // 1. SIGINT — polite interrupt
    if let Err(e) = kill(nix_pid, Signal::SIGINT) {
        tracing::debug!(error = %e, pid, "SIGINT failed (process may have exited)");
        return;
    }
    tracing::debug!(pid, "sent SIGINT");

    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // Check if still alive
    if kill(nix_pid, None).is_err() {
        tracing::debug!(pid, "process exited after SIGINT");
        return;
    }

    // 2. SIGTERM — firm request
    if let Err(e) = kill(nix_pid, Signal::SIGTERM) {
        tracing::debug!(error = %e, pid, "SIGTERM failed");
        return;
    }
    tracing::debug!(pid, "sent SIGTERM");

    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    // Check if still alive
    if kill(nix_pid, None).is_err() {
        tracing::debug!(pid, "process exited after SIGTERM");
        return;
    }

    // 3. SIGKILL — force kill
    if let Err(e) = kill(nix_pid, Signal::SIGKILL) {
        tracing::debug!(error = %e, pid, "SIGKILL failed");
        return;
    }
    tracing::info!(
        pid,
        "sent SIGKILL (process did not respond to SIGINT/SIGTERM)"
    );
}

/// Kill process and all children (best-effort).
#[tracing::instrument(name = "sandbox::kill_process_tree", fields(pid))]
pub(crate) async fn kill_process_tree(pid: u32) {
    let nix_pid = Pid::from_raw(pid as i32);

    // Send SIGKILL to the process group (negative PID)
    let pgid = Pid::from_raw(-(pid as i32));
    let _ = kill(pgid, Signal::SIGKILL);

    // Also kill the process directly as fallback
    let _ = kill(nix_pid, Signal::SIGKILL);
}
