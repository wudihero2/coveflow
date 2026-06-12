pub type SandboxResult<T> = Result<T, SandboxError>;

#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("execution timed out after {0}s")]
    Timeout(u32),
    #[error("run canceled: {0}")]
    Canceled(String),
    #[error("process killed by signal {0}")]
    Killed(i32),
    #[error("execution failed: {0}")]
    ExecutionErr(String),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Other(String),
}
