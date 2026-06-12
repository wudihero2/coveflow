use std::path::Path;

use crate::error::SandboxResult;

#[tracing::instrument(name = "common.write_file", skip(content), fields(path))]
pub fn write_file(run_dir: &str, filename: &str, content: &str) -> SandboxResult<()> {
    let path = Path::new(run_dir).join(filename);
    tracing::Span::current().record("path", path.display().to_string().as_str());
    std::fs::write(&path, content)?;
    tracing::debug!(filename, bytes = content.len(), "file written");
    Ok(())
}
