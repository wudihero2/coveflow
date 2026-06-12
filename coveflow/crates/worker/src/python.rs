use std::collections::HashMap;

use coveflow_types::run::Run;
use tokio_util::sync::CancellationToken;

use crate::common::write_file;
use crate::error::{SandboxError, SandboxResult};
use crate::sandbox::{Sandbox, SandboxContext, SandboxResources, ScriptLang};

const MAX_RESULT_BYTES: usize = 10 * 1024 * 1024; // 10 MB

const BOOTSTRAP: &str = r#"
import builtins, importlib.util, inspect, json, sys, traceback
# Save real stdout for JSON result output, then redirect stdout to stderr
# so that user's print() calls don't corrupt the JSON result on stdout.
_out = sys.stdout
sys.stdout = sys.stderr
payload = json.loads(sys.stdin.buffer.read().decode())
args = payload.get("args", {})
ctx = payload.get("ctx", {})

# Secret SDK: expose `Secret.get("path"[, default])` as a builtin (zero import)
# BEFORE importing the user module, so module-level code can use it too. Values
# come from the stdin payload (no network); pop them off so they don't linger on
# `payload`.
class SecretNotFound(KeyError):
    pass
class _Secret:
    def __init__(self, store):
        self._store = store
    def get(self, path, *default):
        if path in self._store:
            return self._store[path]
        if default:
            return default[0]
        raise SecretNotFound(path)
builtins.Secret = _Secret(payload.pop("secrets", {}))
builtins.SecretNotFound = SecretNotFound

spec = importlib.util.spec_from_file_location("_run", "main.py")
mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)
# Airflow-style signature-aware injection: only pass the execution context when
# main() declares a `ctx` parameter or accepts **kwargs. Existing scripts like
# `def main(name):` are unaffected — they never receive ctx.
sig = inspect.signature(mod.main)
has_var_kw = any(p.kind == p.VAR_KEYWORD for p in sig.parameters.values())
if "ctx" in sig.parameters or has_var_kw:
    args = {**args, "ctx": ctx}
try:
    result = mod.main(**args)
    json.dump(result, _out, default=str)
except Exception as e:
    err = {"error": {"message": str(e), "name": type(e).__name__, "stack_trace": traceback.format_exc()}}
    json.dump(err, _out)
    sys.exit(1)
"#;

#[tracing::instrument(
    name = "python.exec",
    skip(run, content, secrets, sandbox, resource_limits, cancel_token),
    fields(
        run_id = %run.id,
        run_dir,
        exit_code = tracing::field::Empty,
    )
)]
#[allow(clippy::too_many_arguments)]
pub async fn exec_python(
    run: &Run,
    content: &str,
    run_dir: &str,
    run_context: &serde_json::Value,
    secrets: &HashMap<String, String>,
    sandbox: &dyn Sandbox,
    resource_limits: SandboxResources,
    cancel_token: CancellationToken,
) -> SandboxResult<serde_json::Value> {
    if !run.requirements.is_empty() {
        install_python_deps(
            run.id,
            &run.requirements,
            run_dir,
            run.custom_image.clone(),
            &resource_limits,
            sandbox,
            cancel_token.clone(),
        )
        .await?;
    }

    write_file(run_dir, "main.py", content)?;

    // stdin payload: user args + the Airflow-style execution context. The python
    // wrapper decides (by main()'s signature) whether to actually pass ctx in.
    let args_value = run.args.as_ref().cloned().unwrap_or(serde_json::json!({}));
    let payload = serde_json::json!({ "args": args_value, "ctx": run_context, "secrets": secrets });
    let args_json = serde_json::to_vec(&payload)?;

    let ctx = SandboxContext {
        run_id: run.id,
        run_dir: run_dir.to_string(),
        command: "python3".to_string(),
        args: vec!["-c".to_string(), BOOTSTRAP.to_string()],
        env: build_reserved_env(run),
        timeout_secs: run.timeout.unwrap_or(3600) as u32,
        language: ScriptLang::Python3,
        custom_image: run.custom_image.clone(),
        trace_context: None,
        resource_limits,
        stdin: Some(args_json),
        // User scripts must never see the host network. Egress is a future
        // workspace-level policy decision; default is hard-isolated.
        allow_network: false,
    };

    let result = sandbox.execute(&ctx, cancel_token).await?;

    let span = tracing::Span::current();
    span.record("exit_code", result.exit_code);

    if result.stdout.len() > MAX_RESULT_BYTES {
        return Err(SandboxError::Other("result exceeds 10MB limit".into()));
    }

    if result.exit_code != 0 {
        if !result.stdout.is_empty() {
            if let Ok(error_value) = serde_json::from_str::<serde_json::Value>(&result.stdout) {
                tracing::warn!(
                    exit_code = result.exit_code,
                    stderr = %result.stderr,
                    "python execution failed with structured error"
                );
                return Ok(error_value);
            }
        }
        tracing::warn!(
            exit_code = result.exit_code,
            stderr = %result.stderr,
            "python execution failed"
        );
        return Err(SandboxError::ExecutionErr(result.stderr));
    }

    if !result.stderr.is_empty() {
        tracing::info!(stderr = %result.stderr, "user output from script");
    }
    tracing::info!(exit_code = result.exit_code, "python execution completed");

    let value: serde_json::Value = serde_json::from_str(&result.stdout)?;
    Ok(value)
}

#[tracing::instrument(
    name = "python.install_deps",
    skip(sandbox, cancel_token, run_resources),
    fields(
        package_count = requirements.len(),
        exit_code = tracing::field::Empty,
    )
)]
async fn install_python_deps(
    run_id: uuid::Uuid,
    requirements: &[String],
    run_dir: &str,
    custom_image: Option<String>,
    run_resources: &SandboxResources,
    sandbox: &dyn Sandbox,
    cancel_token: CancellationToken,
) -> SandboxResult<()> {
    tracing::info!(packages = ?requirements, "installing python dependencies");

    let mut pip_args = vec![
        "-m".to_string(),
        "pip".to_string(),
        "install".to_string(),
        "--target".to_string(),
        ".".to_string(),
        "--no-input".to_string(),
        "--no-color".to_string(),
    ];
    pip_args.extend(requirements.iter().cloned());

    // Install reuses the run's cpu/memory/disk quota; only the time budget is
    // narrower because installs should never hit a 1h run timeout.
    let install_resources = SandboxResources {
        cpu: run_resources.cpu,
        memory_bytes: run_resources.memory_bytes,
        disk_bytes: run_resources.disk_bytes,
        timeout_secs: 300,
    };

    // pip downloads wheels to TMPDIR before installing. Inside the jail, /tmp
    // comes from the rootfs and may be tiny or read-only — point pip at /work
    // (the bind-mounted run_dir, sized by disk_mb) so large packages succeed.
    let mut pip_env = HashMap::new();
    pip_env.insert("TMPDIR".to_string(), "/work".to_string());
    pip_env.insert("PIP_CACHE_DIR".to_string(), "/work/.pip-cache".to_string());

    let ctx = SandboxContext {
        run_id,
        run_dir: run_dir.to_string(),
        command: "python3".to_string(),
        args: pip_args,
        env: pip_env,
        timeout_secs: 300,
        language: ScriptLang::Python3,
        custom_image,
        trace_context: None,
        resource_limits: install_resources,
        stdin: None,
        // pip install must reach the package index. This is the one stage
        // that intentionally opts in to host network access.
        allow_network: true,
    };

    let result = sandbox.execute(&ctx, cancel_token).await?;

    let span = tracing::Span::current();
    span.record("exit_code", result.exit_code);

    if result.exit_code != 0 {
        tracing::error!(
            exit_code = result.exit_code,
            stderr = %result.stderr,
            "pip install failed"
        );
        return Err(SandboxError::ExecutionErr(format!(
            "pip install failed (exit {}): {}",
            result.exit_code, result.stderr
        )));
    }

    if !result.stdout.is_empty() {
        tracing::info!(stdout = %result.stdout, "pip output (stdout)");
    }
    if !result.stderr.is_empty() {
        tracing::info!(stderr = %result.stderr, "pip output (stderr)");
    }

    tracing::info!("python dependencies installed successfully");
    Ok(())
}

fn build_reserved_env(run: &Run) -> HashMap<String, String> {
    let mut env = HashMap::new();
    env.insert("CF_RUN_ID".to_string(), run.id.to_string());
    env.insert("CF_WORKSPACE".to_string(), run.workspace_id.clone());
    env.insert("CF_TAG".to_string(), run.tag.clone());
    env.insert("PYTHONPATH".to_string(), ".".to_string());

    if let Some(path) = &run.script_path {
        env.insert("CF_SCRIPT_PATH".to_string(), path.clone());
    }
    if let Some(hash) = &run.script_hash {
        env.insert("CF_SCRIPT_HASH".to_string(), hash.clone());
    }

    env
}

#[cfg(test)]
mod tests {
    use super::*;
    use coveflow_types::RunKind;

    /// Initialize tracing for tests. Safe to call multiple times (only the first call takes effect).
    /// Usage: `RUST_LOG=debug cargo test -- --nocapture`
    fn init_tracing() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_test_writer()
            .try_init();
    }

    #[test]
    fn test_bootstrap_code_content() {
        assert!(BOOTSTRAP.contains("importlib.util"));
        assert!(BOOTSTRAP.contains("sys.stdout = sys.stderr"));
        assert!(BOOTSTRAP.contains("main.py"));
        assert!(BOOTSTRAP.contains("sys.stdin.buffer.read()"));
        assert!(BOOTSTRAP.contains("mod.main(**args)"));
        assert!(BOOTSTRAP.contains("json.dump(result, _out, default=str)"));
        assert!(BOOTSTRAP.contains("sys.exit(1)"));
        assert!(BOOTSTRAP.contains("traceback.format_exc()"));
    }

    #[test]
    fn test_build_reserved_env_basic() {
        let run = Run {
            id: uuid::Uuid::nil(),
            workspace_id: "test-ws".to_string(),
            kind: RunKind::Script,
            script_hash: Some("abc123".to_string()),
            script_path: Some("users/alice/my_script".to_string()),
            raw_code: None,
            language: Some(ScriptLang::Python3),
            args: None,
            tag: "default".to_string(),
            parent_run: None,
            root_run: None,
            requirements: vec![],
            timeout: None,
            custom_image: None,
            created_by: "alice@example.com".to_string(),
            trace_id: None,
            span_id: None,
        };

        let env = build_reserved_env(&run);
        assert_eq!(
            env.get("CF_RUN_ID").expect("CF_RUN_ID missing"),
            "00000000-0000-0000-0000-000000000000"
        );
        assert_eq!(
            env.get("CF_WORKSPACE").expect("CF_WORKSPACE missing"),
            "test-ws"
        );
        assert_eq!(env.get("CF_TAG").expect("CF_TAG missing"), "default");
        assert_eq!(
            env.get("CF_SCRIPT_PATH").expect("CF_SCRIPT_PATH missing"),
            "users/alice/my_script"
        );
        assert_eq!(
            env.get("CF_SCRIPT_HASH").expect("CF_SCRIPT_HASH missing"),
            "abc123"
        );
        assert_eq!(env.get("PYTHONPATH").expect("PYTHONPATH missing"), ".");
    }

    #[test]
    fn test_build_reserved_env_no_script() {
        let run = Run {
            id: uuid::Uuid::nil(),
            workspace_id: "ws".to_string(),
            kind: RunKind::Preview,
            script_hash: None,
            script_path: None,
            raw_code: Some("print('hi')".to_string()),
            language: Some(ScriptLang::Python3),
            args: None,
            tag: "dev".to_string(),
            parent_run: None,
            root_run: None,
            requirements: vec![],
            timeout: None,
            custom_image: None,
            created_by: "bob@example.com".to_string(),
            trace_id: None,
            span_id: None,
        };

        let env = build_reserved_env(&run);
        assert_eq!(env.get("CF_TAG").expect("CF_TAG missing"), "dev");
        assert!(env.get("CF_SCRIPT_PATH").is_none());
        assert!(env.get("CF_SCRIPT_HASH").is_none());
    }

    #[tokio::test]
    async fn test_exec_python_simple() {
        use crate::sandbox::none::NoneSandbox;

        let tmp = tempfile::tempdir().expect("failed to create temp dir");
        let run_dir = tmp.path().to_string_lossy().to_string();

        let run = Run {
            id: uuid::Uuid::new_v4(),
            workspace_id: "test-ws".to_string(),
            kind: RunKind::Script,
            script_hash: None,
            script_path: None,
            raw_code: None,
            language: Some(ScriptLang::Python3),
            args: Some(serde_json::json!({"name": "world"})),
            tag: "none".to_string(),
            parent_run: None,
            root_run: None,
            requirements: vec![],
            timeout: Some(30),
            custom_image: None,
            created_by: "test@example.com".to_string(),
            trace_id: None,
            span_id: None,
        };

        let content = r#"
def main(name="default"):
    return {"greeting": f"hello {name}"}
"#;

        let sandbox = NoneSandbox;
        let result = exec_python(
            &run,
            content,
            &run_dir,
            &serde_json::json!({}),
            &HashMap::new(),
            &sandbox,
            SandboxResources::default(),
            CancellationToken::new(),
        )
        .await;

        match result {
            Ok(value) => {
                assert_eq!(value["greeting"], "hello world");
            }
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("No such file or directory") || err_str.contains("not found") {
                    eprintln!("python3 not available, skipping integration test");
                    return;
                }
                panic!("unexpected error: {e}");
            }
        }
    }

    #[tokio::test]
    async fn test_exec_python_large_args() {
        use crate::sandbox::none::NoneSandbox;

        let tmp = tempfile::tempdir().expect("failed to create temp dir");
        let run_dir = tmp.path().to_string_lossy().to_string();

        // Build a large list (10,000 items) to exceed typical pipe buffer (64KB)
        let items: Vec<i64> = (0..10_000).collect();
        let args = serde_json::json!({ "data": items });

        let run = Run {
            id: uuid::Uuid::new_v4(),
            workspace_id: "test-ws".to_string(),
            kind: RunKind::Script,
            script_hash: None,
            script_path: None,
            raw_code: None,
            language: Some(ScriptLang::Python3),
            args: Some(args),
            tag: "none".to_string(),
            parent_run: None,
            root_run: None,
            requirements: vec![],
            timeout: Some(30),
            custom_image: None,
            created_by: "test@example.com".to_string(),
            trace_id: None,
            span_id: None,
        };

        let content = r#"
def main(data):
    return {"count": len(data), "sum": sum(data)}
"#;

        let sandbox = NoneSandbox;
        let result = exec_python(
            &run,
            content,
            &run_dir,
            &serde_json::json!({}),
            &HashMap::new(),
            &sandbox,
            SandboxResources::default(),
            CancellationToken::new(),
        )
        .await;

        match result {
            Ok(value) => {
                assert_eq!(value["count"], 10_000);
                assert_eq!(value["sum"], (0i64..10_000).sum::<i64>());
            }
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("No such file or directory") || err_str.contains("not found") {
                    eprintln!("python3 not available, skipping integration test");
                    return;
                }
                panic!("unexpected error: {e}");
            }
        }
    }

    #[tokio::test]
    async fn test_exec_python_with_empty_args() {
        use crate::sandbox::none::NoneSandbox;

        let tmp = tempfile::tempdir().expect("failed to create temp dir");
        let run_dir = tmp.path().to_string_lossy().to_string();

        let run = Run {
            id: uuid::Uuid::new_v4(),
            workspace_id: "test-ws".to_string(),
            kind: RunKind::Preview,
            script_hash: None,
            script_path: None,
            raw_code: None,
            language: Some(ScriptLang::Python3),
            args: None,
            tag: "none".to_string(),
            parent_run: None,
            root_run: None,
            requirements: vec![],
            timeout: Some(30),
            custom_image: None,
            created_by: "test@example.com".to_string(),
            trace_id: None,
            span_id: None,
        };

        let content = r#"
def main():
    return {"status": "ok", "value": 42}
"#;

        let sandbox = NoneSandbox;
        let result = exec_python(
            &run,
            content,
            &run_dir,
            &serde_json::json!({}),
            &HashMap::new(),
            &sandbox,
            SandboxResources::default(),
            CancellationToken::new(),
        )
        .await;

        match result {
            Ok(value) => {
                assert_eq!(value["status"], "ok");
                assert_eq!(value["value"], 42);
            }
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("No such file or directory") || err_str.contains("not found") {
                    eprintln!("python3 not available, skipping integration test");
                    return;
                }
                panic!("unexpected error: {e}");
            }
        }
    }

    #[tokio::test]
    async fn test_exec_python_print_goes_to_stderr() {
        use crate::sandbox::none::NoneSandbox;

        let tmp = tempfile::tempdir().expect("failed to create temp dir");
        let run_dir = tmp.path().to_string_lossy().to_string();

        let run = Run {
            id: uuid::Uuid::new_v4(),
            workspace_id: "test-ws".to_string(),
            kind: RunKind::Script,
            script_hash: None,
            script_path: None,
            raw_code: None,
            language: Some(ScriptLang::Python3),
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
        };

        // print() should go to stderr, not corrupt stdout JSON
        let content = r#"
def main():
    print("log message from user code")
    return {"ok": True}
"#;

        let sandbox = NoneSandbox;
        let result = exec_python(
            &run,
            content,
            &run_dir,
            &serde_json::json!({}),
            &HashMap::new(),
            &sandbox,
            SandboxResources::default(),
            CancellationToken::new(),
        )
        .await;

        match result {
            Ok(value) => {
                assert_eq!(value["ok"], true);
            }
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("No such file or directory") || err_str.contains("not found") {
                    eprintln!("python3 not available, skipping integration test");
                    return;
                }
                panic!("unexpected error: {e}");
            }
        }
    }

    #[cfg(target_os = "linux")]
    fn unbounded_test_resources() -> SandboxResources {
        SandboxResources {
            cpu: 0.0,
            memory_bytes: 0,
            disk_bytes: 0,
            timeout_secs: 300,
        }
    }

    fn pip_skip_error(err_str: &str) -> bool {
        err_str.contains("No such file or directory")
            || err_str.contains("not found")
            || err_str.contains("pip install failed")
    }

    /// Helper: test pip install + import with any sandbox implementation.
    async fn assert_pip_install_works(
        sandbox: &dyn Sandbox,
        resources: SandboxResources,
        allow_pip_infra_skip: bool,
    ) {
        init_tracing();

        let tmp = tempfile::tempdir().expect("failed to create temp dir");
        let run_dir = tmp.path().to_string_lossy().to_string();

        let run = Run {
            id: uuid::Uuid::new_v4(),
            workspace_id: "test-ws".to_string(),
            kind: RunKind::Script,
            script_hash: None,
            script_path: None,
            raw_code: None,
            language: Some(ScriptLang::Python3),
            args: Some(serde_json::json!({})),
            tag: "none".to_string(),
            parent_run: None,
            root_run: None,
            requirements: vec!["six".to_string(), "pip-install-test".to_string()],
            timeout: Some(120),
            custom_image: None,
            created_by: "test@example.com".to_string(),
            trace_id: None,
            span_id: None,
        };

        let content = r#"
def main():
    import six
    import pip_install_test
    return {"six_version": six.__version__, "ok": True}
"#;

        let result = exec_python(
            &run,
            content,
            &run_dir,
            &serde_json::json!({}),
            &HashMap::new(),
            sandbox,
            resources,
            CancellationToken::new(),
        )
        .await;

        match result {
            Ok(value) => {
                assert_eq!(value["ok"], true);
                assert!(value["six_version"].is_string());
            }
            Err(e) => {
                let err_str = e.to_string();
                if (allow_pip_infra_skip && pip_skip_error(&err_str))
                    || err_str.contains("No such file or directory")
                    || err_str.contains("not found")
                {
                    eprintln!("python3/pip/network not available, skipping test");
                    return;
                }
                panic!("unexpected error: {e}");
            }
        }
    }

    /// Helper: test pip install failure with any sandbox implementation.
    async fn assert_pip_install_failure(sandbox: &dyn Sandbox) {
        init_tracing();

        let tmp = tempfile::tempdir().expect("failed to create temp dir");
        let run_dir = tmp.path().to_string_lossy().to_string();

        let run = Run {
            id: uuid::Uuid::new_v4(),
            workspace_id: "test-ws".to_string(),
            kind: RunKind::Script,
            script_hash: None,
            script_path: None,
            raw_code: None,
            language: Some(ScriptLang::Python3),
            args: Some(serde_json::json!({})),
            tag: "none".to_string(),
            parent_run: None,
            root_run: None,
            requirements: vec!["this-package-does-not-exist-xyz-999".to_string()],
            timeout: Some(60),
            custom_image: None,
            created_by: "test@example.com".to_string(),
            trace_id: None,
            span_id: None,
        };

        let content = "def main():\n    return {}";

        let result = exec_python(
            &run,
            content,
            &run_dir,
            &serde_json::json!({}),
            &HashMap::new(),
            sandbox,
            SandboxResources::default(),
            CancellationToken::new(),
        )
        .await;

        match result {
            Err(SandboxError::ExecutionErr(msg)) => {
                assert!(
                    msg.contains("pip install failed"),
                    "expected pip failure message, got: {msg}"
                );
            }
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("No such file or directory") || err_str.contains("not found") {
                    eprintln!("python3/pip not available, skipping test");
                    return;
                }
                panic!("expected ExecutionErr, got: {e}");
            }
            Ok(value) => {
                panic!("expected pip failure, got success: {value}");
            }
        }
    }

    #[tokio::test]
    async fn test_pip_install_none_sandbox() {
        use crate::sandbox::none::NoneSandbox;
        assert_pip_install_works(&NoneSandbox, SandboxResources::default(), true).await;
    }

    #[tokio::test]
    async fn test_pip_install_failure_none_sandbox() {
        use crate::sandbox::none::NoneSandbox;
        assert_pip_install_failure(&NoneSandbox).await;
    }

    #[cfg(target_os = "linux")]
    fn nsjail_sandbox_for_pip_test() -> Option<crate::sandbox::nsjail::NsjailSandbox> {
        use crate::config::NsjailConfig;
        use std::path::Path;

        let mut config = NsjailConfig::default();
        config.cgroup_pids_max = 0;

        if !Path::new(&config.nsjail_path).exists() {
            eprintln!("nsjail not available, skipping test");
            return None;
        }

        let Some(runtime) = config.catalog.runtimes.get(&config.catalog.default_runtime) else {
            eprintln!("default nsjail runtime not configured, skipping test");
            return None;
        };

        if !Path::new(&runtime.rootfs).exists() {
            eprintln!("default nsjail rootfs not available, skipping test");
            return None;
        }

        Some(crate::sandbox::nsjail::NsjailSandbox::new(config))
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    #[ignore = "requires nsjail, Python rootfs, and package index access"]
    async fn test_pip_install_nsjail_sandbox() {
        let Some(sandbox) = nsjail_sandbox_for_pip_test() else {
            return;
        };

        assert_pip_install_works(&sandbox, unbounded_test_resources(), false).await;
    }

    // Future sandbox tests:
    // async fn test_pip_install_k8s_sandbox() { assert_pip_install_works(&K8sSandbox::new(cfg)).await; }

    /// Run `content`'s `main` once with the given args + ctx. Returns `None`
    /// (and skips) when python3 is unavailable, like the other exec tests.
    async fn run_main(
        content: &str,
        args: serde_json::Value,
        ctx: serde_json::Value,
    ) -> Option<serde_json::Value> {
        run_main_with_secrets(content, args, ctx, HashMap::new()).await
    }

    async fn run_main_with_secrets(
        content: &str,
        args: serde_json::Value,
        ctx: serde_json::Value,
        secrets: HashMap<String, String>,
    ) -> Option<serde_json::Value> {
        use crate::sandbox::none::NoneSandbox;
        let tmp = tempfile::tempdir().expect("failed to create temp dir");
        let run_dir = tmp.path().to_string_lossy().to_string();
        let run = Run {
            id: uuid::Uuid::new_v4(),
            workspace_id: "test-ws".to_string(),
            kind: RunKind::Script,
            script_hash: None,
            script_path: None,
            raw_code: None,
            language: Some(ScriptLang::Python3),
            args: Some(args),
            tag: "none".to_string(),
            parent_run: None,
            root_run: None,
            requirements: vec![],
            timeout: Some(30),
            custom_image: None,
            created_by: "test@example.com".to_string(),
            trace_id: None,
            span_id: None,
        };
        let result = exec_python(
            &run,
            content,
            &run_dir,
            &ctx,
            &secrets,
            &NoneSandbox,
            SandboxResources::default(),
            CancellationToken::new(),
        )
        .await;
        match result {
            Ok(v) => Some(v),
            Err(e) => {
                let s = e.to_string();
                if s.contains("No such file or directory") || s.contains("not found") {
                    eprintln!("python3 not available, skipping signature test");
                    None
                } else {
                    panic!("unexpected error: {e}");
                }
            }
        }
    }

    /// R3: the python wrapper injects `ctx` only when `main` asks for it, so
    /// existing signatures keep working unchanged.
    #[tokio::test]
    async fn test_signature_aware_ctx_injection() {
        let ctx = serde_json::json!({ "ds": "2026-06-10", "run_id": "abc" });

        // 1. `def main(ctx):` receives the whole context dict.
        if let Some(v) = run_main(
            "def main(ctx):\n    return {\"ds\": ctx[\"ds\"]}\n",
            serde_json::json!({}),
            ctx.clone(),
        )
        .await
        {
            assert_eq!(v["ds"], "2026-06-10");
        }

        // 2. `def main(name):` is unaffected — passing ctx would raise TypeError.
        if let Some(v) = run_main(
            "def main(name):\n    return {\"hi\": name}\n",
            serde_json::json!({ "name": "stan" }),
            ctx.clone(),
        )
        .await
        {
            assert_eq!(v["hi"], "stan");
        }

        // 3. `def main(name, **kw):` receives ctx via kwargs.
        if let Some(v) = run_main(
            "def main(name, **kw):\n    return {\"hi\": name, \"rid\": kw[\"ctx\"][\"run_id\"]}\n",
            serde_json::json!({ "name": "stan" }),
            ctx.clone(),
        )
        .await
        {
            assert_eq!(v["hi"], "stan");
            assert_eq!(v["rid"], "abc");
        }
    }

    /// R4: the `Secret` builtin resolves injected secrets, returns a default
    /// when provided, and raises `SecretNotFound` otherwise — all with zero import.
    #[tokio::test]
    async fn test_secret_sdk_get_default_and_raise() {
        let mut secrets = HashMap::new();
        secrets.insert("workspace/openai".to_string(), "sk-live".to_string());

        // Hit.
        if let Some(v) = run_main_with_secrets(
            "def main():\n    return {\"k\": Secret.get(\"workspace/openai\")}\n",
            serde_json::json!({}),
            serde_json::json!({}),
            secrets.clone(),
        )
        .await
        {
            assert_eq!(v["k"], "sk-live");
        }

        // Default when missing.
        if let Some(v) = run_main_with_secrets(
            "def main():\n    return {\"k\": Secret.get(\"workspace/missing\", \"fallback\")}\n",
            serde_json::json!({}),
            serde_json::json!({}),
            secrets.clone(),
        )
        .await
        {
            assert_eq!(v["k"], "fallback");
        }

        // Raise SecretNotFound when missing and no default (caught by the user).
        if let Some(v) = run_main_with_secrets(
            "def main():\n    try:\n        Secret.get(\"workspace/missing\")\n    except SecretNotFound:\n        return {\"raised\": True}\n",
            serde_json::json!({}),
            serde_json::json!({}),
            secrets,
        )
        .await
        {
            assert_eq!(v["raised"], true);
        }
    }

    /// A run with no readable secrets sees an empty store — `Secret.get` raises.
    #[tokio::test]
    async fn test_secret_sdk_empty_store_raises() {
        if let Some(v) = run_main_with_secrets(
            "def main():\n    try:\n        Secret.get(\"workspace/x\")\n        return {\"raised\": False}\n    except SecretNotFound:\n        return {\"raised\": True}\n",
            serde_json::json!({}),
            serde_json::json!({}),
            HashMap::new(),
        )
        .await
        {
            assert_eq!(v["raised"], true);
        }
    }

    #[test]
    fn test_result_size_limit() {
        assert_eq!(MAX_RESULT_BYTES, 10 * 1024 * 1024);
    }
}
