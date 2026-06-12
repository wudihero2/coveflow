//! Resolve + decrypt the secrets a run may use, for injection into the sandbox.
//!
//! The sandbox has no network, so `Secret.get` can't call back to the API.
//! Instead, before executing user code the worker decrypts every secret the
//! run's creator can read (the shared three-root rule in
//! [`coveflow_types::permissions::secret_readable`]) and passes them in via the
//! stdin payload.
//!
//! SECURITY: plaintext values live only in the returned map (→ stdin payload →
//! child process memory). They are never logged; a decrypt failure reports only
//! the offending path, never the value or key.

use std::collections::HashMap;

use coveflow_types::crypto::{self, SecretKey};
use coveflow_types::permissions::secret_readable;
use sqlx::PgPool;

use crate::error::{SandboxError, SandboxResult};

/// Decrypt the secrets readable by `created_by` in `workspace_id`, keyed by path.
///
/// A decryption failure fails the whole resolution (and therefore the run): it
/// signals key drift or tampering, which must surface loudly rather than run
/// user code with a silently-missing secret.
///
/// `skip(key)` keeps the master key out of the span; `count` records only how
/// many secrets were injected, never their paths or values.
#[tracing::instrument(
    name = "worker::resolve_secrets",
    skip(db, key),
    fields(%workspace_id, count = tracing::field::Empty)
)]
pub async fn resolve_secrets(
    db: &PgPool,
    key: &SecretKey,
    workspace_id: &str,
    created_by: &str,
) -> SandboxResult<HashMap<String, String>> {
    let teams: Vec<String> = sqlx::query_scalar!(
        "SELECT team_name FROM team_member WHERE workspace_id = $1 AND email = $2",
        workspace_id,
        created_by,
    )
    .fetch_all(db)
    .await
    .map_err(|e| SandboxError::Other(format!("failed to load team membership: {e}")))?;

    let rows = sqlx::query!(
        "SELECT path, value_encrypted FROM secret WHERE workspace_id = $1",
        workspace_id,
    )
    .fetch_all(db)
    .await
    .map_err(|e| SandboxError::Other(format!("failed to load secrets: {e}")))?;

    let mut out = HashMap::new();
    for row in rows {
        if !secret_readable(created_by, &teams, &row.path) {
            continue;
        }
        let plaintext = crypto::decrypt(key, &row.value_encrypted)
            // Note: only the path, never the value or key.
            .map_err(|_| {
                SandboxError::Other(format!("decrypt failed for secret '{}'", row.path))
            })?;
        let value = String::from_utf8(plaintext).map_err(|_| {
            SandboxError::Other(format!("secret '{}' is not valid UTF-8", row.path))
        })?;
        out.insert(row.path, value);
    }
    tracing::Span::current().record("count", out.len());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    fn key() -> SecretKey {
        let b64 = base64::engine::general_purpose::STANDARD.encode([9u8; 32]);
        SecretKey::from_base64(&b64).unwrap()
    }

    async fn seed_ws(db: &PgPool, ws: &str) {
        sqlx::query!(
            "INSERT INTO workspace (id, name, owner) VALUES ($1, 'T', 'o@x.com')
             ON CONFLICT DO NOTHING",
            ws
        )
        .execute(db)
        .await
        .unwrap();
    }

    async fn put_secret(db: &PgPool, ws: &str, path: &str, value: &[u8], key: &SecretKey) {
        let blob = crypto::encrypt(key, value).unwrap();
        sqlx::query!(
            "INSERT INTO secret (workspace_id, path, value_encrypted, created_by, updated_by)
             VALUES ($1, $2, $3, 'seed@x.com', 'seed@x.com')",
            ws,
            path,
            blob
        )
        .execute(db)
        .await
        .unwrap();
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn resolves_only_readable_secrets(pool: sqlx::PgPool) {
        let k = key();
        seed_ws(&pool, "ws").await;
        sqlx::query!("INSERT INTO team (workspace_id, name, summary) VALUES ('ws', 'data', '')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query!(
            "INSERT INTO team_member (workspace_id, email, team_name) VALUES ('ws', 'a@x.com', 'data')"
        )
        .execute(&pool)
        .await
        .unwrap();

        put_secret(&pool, "ws", "workspace/shared", b"w", &k).await;
        put_secret(&pool, "ws", "users/a@x.com/mine", b"m", &k).await;
        put_secret(&pool, "ws", "users/b@x.com/theirs", b"t", &k).await;
        put_secret(&pool, "ws", "teams/data/dk", b"d", &k).await;
        put_secret(&pool, "ws", "teams/ops/ok", b"o", &k).await;

        let got = resolve_secrets(&pool, &k, "ws", "a@x.com").await.unwrap();

        // workspace (any), own users/, own team — yes; other user + other team — no.
        assert_eq!(got.get("workspace/shared").map(String::as_str), Some("w"));
        assert_eq!(got.get("users/a@x.com/mine").map(String::as_str), Some("m"));
        assert_eq!(got.get("teams/data/dk").map(String::as_str), Some("d"));
        assert!(!got.contains_key("users/b@x.com/theirs"));
        assert!(!got.contains_key("teams/ops/ok"));
        assert_eq!(got.len(), 3);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn decrypt_failure_is_an_error(pool: sqlx::PgPool) {
        seed_ws(&pool, "ws").await;
        // Encrypt with one key, resolve with another → decrypt fails → run fails.
        put_secret(&pool, "ws", "workspace/k", b"v", &key()).await;
        let other = {
            let b64 = base64::engine::general_purpose::STANDARD.encode([1u8; 32]);
            SecretKey::from_base64(&b64).unwrap()
        };
        let err = resolve_secrets(&pool, &other, "ws", "a@x.com")
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("decrypt failed"), "got: {msg}");
        // Never leaks the value.
        assert!(!msg.contains('v') || !msg.contains("value"));
    }
}
