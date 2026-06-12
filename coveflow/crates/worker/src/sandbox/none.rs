use super::{Sandbox, SandboxContext, SandboxOutput, run_child};
use crate::error::SandboxResult;
use async_trait::async_trait;
use coveflow_types::scripts::ALLOWED_PYTHON_RUNTIMES;
use tokio_util::sync::CancellationToken;

pub(crate) struct NoneSandbox;

/// In dev/none mode, runtime selection is a best-effort host binary mapping.
/// This sandbox does not pull or mount runtime images. It only maps allowlisted
/// values such as `python:3.11` to `python3.11` if that binary is already
/// installed where the worker is running.
fn resolve_command(ctx: &SandboxContext) -> String {
    resolve_command_with_probe(ctx, command_exists)
}

fn command_exists(command: &str) -> bool {
    std::process::Command::new(command)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn resolve_command_with_probe(
    ctx: &SandboxContext,
    command_exists: impl Fn(&str) -> bool,
) -> String {
    if let Some(image) = &ctx.custom_image {
        // Only process values that are in the allowlist.
        if !ALLOWED_PYTHON_RUNTIMES.contains(&image.as_str()) {
            tracing::warn!(
                image = %image,
                fallback = %ctx.command,
                "none sandbox: custom_image not in allowlist, using default"
            );
            return ctx.command.clone();
        }

        if let Some(version) = image.strip_prefix("python:") {
            let versioned = format!("python{version}");
            if command_exists(&versioned) {
                tracing::info!(
                    image = %image,
                    resolved = %versioned,
                    "none sandbox: resolved runtime to local binary"
                );
                return versioned;
            }
            tracing::warn!(
                image = %image,
                tried = %versioned,
                fallback = %ctx.command,
                "none sandbox: versioned python binary not found, using default"
            );
        }
    }
    ctx.command.clone()
}

#[async_trait]
impl Sandbox for NoneSandbox {
    #[tracing::instrument(
        name = "sandbox.execute",
        skip(self, ctx, cancel),
        fields(
            run_id = %ctx.run_id,
            sandbox_mode = "none",
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
        let command = resolve_command(ctx);
        let mut cmd = tokio::process::Command::new(&command);
        cmd.current_dir(&ctx.run_dir).args(&ctx.args).envs(&ctx.env);

        let timeout = std::time::Duration::from_secs(ctx.timeout_secs as u64);
        run_child(cmd, ctx, cancel, timeout, "none", None).await
    }

    async fn health_check(&self) -> SandboxResult<()> {
        Ok(())
    }

    fn name(&self) -> &str {
        "none"
    }

    fn supports_resource_limits(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::*;
    use std::collections::HashMap;

    fn make_ctx(command: &str, args: Vec<&str>, timeout_secs: u32) -> SandboxContext {
        SandboxContext {
            run_id: uuid::Uuid::new_v4(),
            run_dir: std::env::temp_dir().to_string_lossy().into_owned(),
            command: command.to_string(),
            args: args.into_iter().map(String::from).collect(),
            env: HashMap::new(),
            timeout_secs,
            language: ScriptLang::Python3,
            custom_image: None,
            trace_context: None,
            resource_limits: SandboxResources {
                cpu: 1.0,
                memory_bytes: 512 * 1024 * 1024,
                disk_bytes: 1024 * 1024 * 1024,
                timeout_secs,
            },
            stdin: None,
            allow_network: false,
        }
    }

    fn make_ctx_with_image(custom_image: Option<&str>) -> SandboxContext {
        let mut ctx = make_ctx("python3", Vec::new(), 5);
        ctx.custom_image = custom_image.map(String::from);
        ctx
    }

    #[test]
    fn test_name() {
        let sandbox = NoneSandbox;
        assert_eq!(sandbox.name(), "none");
    }

    #[test]
    fn test_supports_resource_limits() {
        let sandbox = NoneSandbox;
        assert!(!sandbox.supports_resource_limits());
    }

    #[tokio::test]
    async fn test_health_check() {
        let sandbox = NoneSandbox;
        assert!(sandbox.health_check().await.is_ok());
    }

    #[test]
    fn test_resolve_command_uses_default_without_custom_image() {
        let ctx = make_ctx_with_image(None);
        let command = resolve_command_with_probe(&ctx, |_| {
            panic!("probe should not run without custom_image")
        });

        assert_eq!(command, "python3");
    }

    #[test]
    fn test_resolve_command_rejects_disallowed_custom_image() {
        let ctx = make_ctx_with_image(Some("python:9.99"));
        let command = resolve_command_with_probe(&ctx, |_| {
            panic!("probe should not run for disallowed custom_image")
        });

        assert_eq!(command, "python3");
    }

    #[test]
    fn test_resolve_command_uses_allowlisted_local_binary() {
        let ctx = make_ctx_with_image(Some("python:3.11"));
        let command = resolve_command_with_probe(&ctx, |candidate| candidate == "python3.11");

        assert_eq!(command, "python3.11");
    }

    #[test]
    fn test_resolve_command_falls_back_when_allowlisted_binary_is_missing() {
        let ctx = make_ctx_with_image(Some("python:3.11"));
        let command = resolve_command_with_probe(&ctx, |_| false);

        assert_eq!(command, "python3");
    }

    #[tokio::test]
    async fn test_execute_echo() {
        let sandbox = NoneSandbox;
        let ctx = make_ctx("echo", vec!["hello"], 5);
        let output = sandbox
            .execute(&ctx, CancellationToken::new())
            .await
            .expect("execute failed");
        assert_eq!(output.exit_code, 0);
        assert_eq!(output.stdout.trim(), "hello");
    }

    #[tokio::test]
    async fn test_execute_failure() {
        let sandbox = NoneSandbox;
        let ctx = make_ctx("false", vec![], 5);
        let output = sandbox
            .execute(&ctx, CancellationToken::new())
            .await
            .expect("execute failed");
        assert_ne!(output.exit_code, 0);
    }

    #[tokio::test]
    async fn test_execute_timeout() {
        let sandbox = NoneSandbox;
        let ctx = make_ctx("sleep", vec!["10"], 1);
        let result = sandbox.execute(&ctx, CancellationToken::new()).await;
        assert!(matches!(result, Err(SandboxError::Timeout(1))));
    }
}
