//! Three-system-root read rule, shared by the API (`AuthedUser::can_read`) and
//! the worker (secret injection) so the two cannot drift. Pure function, no auth
//! context — the worker resolves a run's `created_by` + team membership itself.

/// The folder segment immediately after `prefix` (e.g. the owner email under
/// `users/`, or the team name under `teams/`).
fn root_name<'a>(path: &'a str, prefix: &str) -> Option<&'a str> {
    path.strip_prefix(prefix)
        .map(|r| r.split('/').next().unwrap_or(""))
}

/// Whether `email` (a member of `teams`) may read a resource at `path` under the
/// three-system-root model:
///   - `workspace/..`     → any member
///   - `users/<email>/..` → only that user
///   - `teams/<t>/..`     → only members of team `t`
///
/// Note: no instance-admin bypass here — `can_read` layers that on top. Callers
/// that need it (the API) check admin separately.
pub fn secret_readable(email: &str, teams: &[String], path: &str) -> bool {
    if let Some(owner) = root_name(path, "users/") {
        return email == owner;
    }
    if let Some(team) = root_name(path, "teams/") {
        return teams.iter().any(|t| t == team);
    }
    path.starts_with("workspace/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_root_is_readable_by_anyone() {
        assert!(secret_readable("a@x.com", &[], "workspace/openai_key"));
    }

    #[test]
    fn user_root_is_owner_only() {
        assert!(secret_readable("a@x.com", &[], "users/a@x.com/key"));
        assert!(!secret_readable("b@x.com", &[], "users/a@x.com/key"));
    }

    #[test]
    fn team_root_requires_membership() {
        let teams = vec!["data".to_string()];
        assert!(secret_readable("a@x.com", &teams, "teams/data/key"));
        assert!(!secret_readable("a@x.com", &teams, "teams/ops/key"));
        assert!(!secret_readable("a@x.com", &[], "teams/data/key"));
    }

    #[test]
    fn unknown_root_is_not_readable() {
        assert!(!secret_readable("a@x.com", &[], "foo/bar"));
    }
}
