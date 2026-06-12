#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, sqlx::Type)]
#[sqlx(type_name = "varchar", rename_all = "snake_case")]
pub enum ScriptLang {
    #[serde(rename = "python3")]
    #[sqlx(rename = "python3")]
    Python3,
}

impl ScriptLang {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Python3 => "python3",
        }
    }
}

impl std::fmt::Display for ScriptLang {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Allowed Python runtime image tags.
/// Used for API validation and worker command resolution.
/// Format: "python:X.Y" where X.Y is a supported CPython version.
pub const ALLOWED_PYTHON_RUNTIMES: &[&str] = &["python:3.11", "python:3.12"];

/// Validate a runtime string. Returns true if the value is in the allowlist
/// or is None (meaning "use platform default").
pub fn is_valid_runtime(runtime: Option<&str>) -> bool {
    match runtime {
        None => true,
        Some(rt) => ALLOWED_PYTHON_RUNTIMES.contains(&rt),
    }
}

/// Trim requirement strings and discard blank entries.
pub fn normalize_requirements(requirements: Option<Vec<String>>) -> Vec<String> {
    requirements
        .unwrap_or_default()
        .into_iter()
        .map(|req| req.trim().to_string())
        .filter(|req| !req.is_empty())
        .collect()
}

impl std::str::FromStr for ScriptLang {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "python3" => Ok(Self::Python3),
            other => Err(format!("invalid script language: {other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_requirements_trims_and_drops_blank_items() {
        let requirements = normalize_requirements(Some(vec![
            " requests==2.31 ".to_string(),
            "".to_string(),
            "   ".to_string(),
            "pandas".to_string(),
        ]));

        assert_eq!(requirements, vec!["requests==2.31", "pandas"]);
    }
}
