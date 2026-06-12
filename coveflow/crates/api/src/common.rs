use sqlx::PgPool;
use uuid::Uuid;

use crate::error::ApiError;

/// Parse a log level string into a numeric value.
/// TRACE=1, DEBUG=2, INFO=3, WARN=4, ERROR=5.
pub fn parse_level(s: &str) -> Option<i16> {
    match s.to_uppercase().as_str() {
        "TRACE" => Some(1),
        "DEBUG" => Some(2),
        "INFO" => Some(3),
        "WARN" | "WARNING" => Some(4),
        "ERROR" => Some(5),
        _ => None,
    }
}

/// Parse a run_id string and verify it belongs to the given workspace.
/// Returns the parsed UUID, or an ApiError if invalid or not found.
pub async fn parse_and_verify_run(
    db: &PgPool,
    workspace_id: &str,
    run_id: &str,
) -> Result<Uuid, ApiError> {
    let run_id: Uuid = run_id
        .parse()
        .map_err(|_| ApiError::BadRequest("invalid run_id".into()))?;

    let run_exists = sqlx::query_scalar!(
        r#"SELECT EXISTS(SELECT 1 FROM run WHERE id = $1 AND workspace_id = $2) as "exists!""#,
        run_id,
        workspace_id,
    )
    .fetch_one(db)
    .await?;

    if !run_exists {
        return Err(ApiError::NotFound);
    }

    Ok(run_id)
}

/// Validate a kebab-case name (teams, folders): `^[a-z0-9][a-z0-9-]*$`, max 100 chars.
pub fn validate_name(name: &str) -> Result<(), ApiError> {
    if name.is_empty() || name.len() > 100 {
        return Err(ApiError::BadRequest("name must be 1-100 characters".into()));
    }
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() => {}
        _ => {
            return Err(ApiError::BadRequest(
                "name must start with a lowercase letter or digit".into(),
            ));
        }
    }
    for c in chars {
        if !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
            return Err(ApiError::BadRequest(
                "name must contain only lowercase letters, digits, and hyphens".into(),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_level_valid() {
        assert_eq!(parse_level("TRACE"), Some(1));
        assert_eq!(parse_level("DEBUG"), Some(2));
        assert_eq!(parse_level("INFO"), Some(3));
        assert_eq!(parse_level("WARN"), Some(4));
        assert_eq!(parse_level("WARNING"), Some(4));
        assert_eq!(parse_level("ERROR"), Some(5));
    }

    #[test]
    fn test_parse_level_case_insensitive() {
        assert_eq!(parse_level("info"), Some(3));
        assert_eq!(parse_level("Info"), Some(3));
        assert_eq!(parse_level("trace"), Some(1));
        assert_eq!(parse_level("warn"), Some(4));
        assert_eq!(parse_level("error"), Some(5));
        assert_eq!(parse_level("Error"), Some(5));
    }

    #[test]
    fn test_parse_level_invalid() {
        assert_eq!(parse_level("FATAL"), None);
        assert_eq!(parse_level(""), None);
        assert_eq!(parse_level("unknown"), None);
        assert_eq!(parse_level("verbose"), None);
    }
}
