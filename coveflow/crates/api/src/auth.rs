use std::collections::HashMap;
use std::sync::LazyLock;

use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::header::{COOKIE, SET_COOKIE};
use axum::http::{HeaderMap, HeaderValue, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::ApiError;

const ACCESS_TOKEN_TTL_SECS: u64 = 900; // 15 minutes
const REFRESH_TOKEN_TTL_SECS: i64 = 604_800; // 7 days

static JWT_SECRET: LazyLock<String> = LazyLock::new(|| {
    std::env::var("JWT_SECRET").unwrap_or_else(|_| {
        tracing::warn!("JWT_SECRET not set, using insecure default (dev only)");
        "coveflow-dev-secret-do-not-use-in-prod".to_string()
    })
});

#[derive(serde::Serialize, serde::Deserialize)]
struct Claims {
    email: String,
    exp: u64,
    iat: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FolderRole {
    Reader,
    Writer,
    Owner,
}

/// Workspace membership role. Matches the CHECK constraint on workspace_member.role.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceRole {
    Admin,
    Editor,
    Viewer,
    Operator,
}

impl WorkspaceRole {
    pub fn is_admin(self) -> bool {
        self == Self::Admin
    }

    pub(crate) fn from_db(s: &str) -> Result<Self, ApiError> {
        match s {
            "admin" => Ok(Self::Admin),
            "editor" => Ok(Self::Editor),
            "viewer" => Ok(Self::Viewer),
            "operator" => Ok(Self::Operator),
            other => Err(ApiError::Internal(format!(
                "unknown workspace role: {other}"
            ))),
        }
    }
}

#[derive(Clone, Debug)]
pub struct AuthedUser {
    pub email: String,
    pub workspace_id: String,
    pub role: WorkspaceRole,
    pub teams: Vec<String>,
    /// Per-team role (reader/writer) for the `teams/<name>/` shared space.
    pub team_roles: HashMap<String, FolderRole>,
}

/// First path segment after a known root prefix, e.g. `users/alice/x` → "alice".
fn root_name<'a>(path: &'a str, prefix: &str) -> Option<&'a str> {
    path.strip_prefix(prefix)
        .map(|r| r.split('/').next().unwrap_or(""))
}

impl AuthedUser {
    pub fn is_admin(&self) -> bool {
        self.role.is_admin()
    }

    // Three roots decide access (admin bypasses all):
    //   users/<email>/…   personal — only that user
    //   teams/<team>/…    team space — members read; writer-role members write
    //   workspace/…       workspace share — members read; non-viewers write
    // Anything else is admin-only (legacy / non-conforming).
    pub fn can_write(&self, path: &str) -> bool {
        if self.is_admin() {
            return true;
        }
        if let Some(owner) = root_name(path, "users/") {
            return self.email == owner;
        }
        if let Some(team) = root_name(path, "teams/") {
            return matches!(
                self.team_roles.get(team),
                Some(FolderRole::Writer | FolderRole::Owner)
            );
        }
        if path.starts_with("workspace/") {
            return self.role != WorkspaceRole::Viewer;
        }
        false
    }

    pub fn can_read(&self, path: &str) -> bool {
        // Instance admins read everything; otherwise the shared three-root rule
        // (kept in coveflow-types so the worker's secret injection matches).
        self.is_admin()
            || coveflow_types::permissions::secret_readable(&self.email, &self.team_names(), path)
    }

    /// The teams this user belongs to (any role), as a plain list for the shared
    /// `secret_readable` rule.
    fn team_names(&self) -> Vec<String> {
        self.team_roles.keys().cloned().collect()
    }

    /// Path must live under one of the three roots: `users/<self>/…`,
    /// `teams/<a team you're in>/…`, or `workspace/…`. Enforced on create so no
    /// one (admins included) invents a fourth top-level folder. A bare root with
    /// no file under it (e.g. `workspace`) is rejected.
    pub fn is_valid_root_path(&self, path: &str) -> bool {
        if let Some(rest) = path.strip_prefix("users/") {
            let (owner, sub) = rest.split_once('/').unwrap_or((rest, ""));
            return !sub.is_empty() && (self.is_admin() || self.email == owner);
        }
        if let Some(rest) = path.strip_prefix("teams/") {
            let (team, sub) = rest.split_once('/').unwrap_or((rest, ""));
            return !sub.is_empty() && (self.is_admin() || self.team_roles.contains_key(team));
        }
        if let Some(rest) = path.strip_prefix("workspace/") {
            return !rest.is_empty();
        }
        false
    }

    pub fn require_writer(&self, path: &str) -> Result<(), ApiError> {
        if self.can_write(path) {
            Ok(())
        } else {
            Err(ApiError::Forbidden(format!("no write access to '{path}'")))
        }
    }

    pub fn require_reader(&self, path: &str) -> Result<(), ApiError> {
        if self.can_read(path) {
            Ok(())
        } else {
            Err(ApiError::Forbidden(format!("no read access to '{path}'")))
        }
    }
}

pub async fn require_auth(
    State(db): State<PgPool>,
    Path(params): Path<HashMap<String, String>>,
    mut req: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, ApiError> {
    let workspace_id = params
        .get("workspace_id")
        .cloned()
        .ok_or_else(|| ApiError::Internal("missing workspace_id in path".into()))?;
    let email = email_from_bearer_headers(req.headers())?;
    let user = authed_user_for(&db, email, workspace_id).await?;

    req.extensions_mut().insert(user);
    Ok(next.run(req).await)
}

/// Build an [`AuthedUser`] for `email` in `workspace_id` (workspace role + team
/// roles). Shared by JWT (`require_auth`) and PAT (webhook) paths so the two
/// produce identical authorization context.
pub(crate) async fn authed_user_for(
    db: &PgPool,
    email: String,
    workspace_id: String,
) -> Result<AuthedUser, ApiError> {
    let member = sqlx::query!(
        "SELECT role FROM workspace_member WHERE workspace_id = $1 AND email = $2",
        workspace_id,
        email
    )
    .fetch_optional(db)
    .await?
    .ok_or(ApiError::Forbidden("not a member of this workspace".into()))?;
    let role = WorkspaceRole::from_db(&member.role)?;

    let team_rows = sqlx::query!(
        "SELECT team_name, role FROM team_member WHERE workspace_id = $1 AND email = $2",
        workspace_id,
        email
    )
    .fetch_all(db)
    .await?;
    let teams: Vec<String> = team_rows.iter().map(|r| r.team_name.clone()).collect();
    let mut team_roles: HashMap<String, FolderRole> = HashMap::new();
    for r in &team_rows {
        let fr = if r.role == "writer" {
            FolderRole::Writer
        } else {
            FolderRole::Reader
        };
        team_roles.insert(r.team_name.clone(), fr);
    }

    Ok(AuthedUser {
        email,
        workspace_id,
        role,
        teams,
        team_roles,
    })
}

/// Extract the raw `Authorization: Bearer <token>` value (not decoded).
pub(crate) fn bearer_token(headers: &HeaderMap) -> Result<String, ApiError> {
    headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|s| s.to_string())
        .ok_or(ApiError::Unauthorized)
}

pub(crate) fn email_from_bearer_headers(headers: &HeaderMap) -> Result<String, ApiError> {
    let token = bearer_token(headers)?;
    Ok(decode::<Claims>(
        &token,
        &DecodingKey::from_secret(JWT_SECRET.as_bytes()),
        &Validation::default(),
    )
    .map_err(|_| ApiError::Unauthorized)?
    .claims
    .email)
}

/// Resolve a Personal API Token (`Authorization: Bearer cf_pat_...`) to its owner
/// email. Returns `Unauthorized` for unknown or expired tokens. Updates
/// `last_used_at` best-effort. SECURITY: never logs the token or its hash.
pub(crate) async fn email_from_pat(db: &PgPool, headers: &HeaderMap) -> Result<String, ApiError> {
    let token = bearer_token(headers)?;
    let hash = coveflow_types::api_token::token_hash(&token);
    let row = sqlx::query!(
        "SELECT email, expires_at FROM api_token WHERE token_hash = $1",
        hash
    )
    .fetch_optional(db)
    .await?
    .ok_or(ApiError::Unauthorized)?;

    if matches!(row.expires_at, Some(exp) if exp < chrono::Utc::now()) {
        return Err(ApiError::Unauthorized);
    }
    // Best-effort: a failed last_used update must not reject a valid token.
    let _ = sqlx::query!(
        "UPDATE api_token SET last_used_at = now() WHERE token_hash = $1",
        hash
    )
    .execute(db)
    .await;

    Ok(row.email)
}

fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    format!("{:x}", hasher.finalize())
}

async fn ensure_default_workspace(db: &PgPool, email: &str) -> Result<(), ApiError> {
    let mut tx = db.begin().await?;
    ensure_default_workspace_membership(&mut tx, email).await?;
    tx.commit().await?;
    Ok(())
}

/// Insert the default workspace + an admin membership for `email` on a caller's
/// transaction, so the membership can be committed atomically with whatever else
/// the caller is doing (e.g. account creation in bootstrap_admin).
///
/// NOTE: membership is created only in the hardcoded `default` workspace. The
/// cluster routes don't rely on workspace membership (they gate on
/// `account.is_admin` directly), but if a future route combines `require_auth`
/// with an instance-admin check, a bootstrapped admin would 403 in any non-default
/// workspace — grant membership explicitly there.
async fn ensure_default_workspace_membership(
    conn: &mut sqlx::PgConnection,
    email: &str,
) -> Result<(), ApiError> {
    sqlx::query!(
        "INSERT INTO workspace (id, name, owner) VALUES ('default', 'Default', $1)
         ON CONFLICT (id) DO NOTHING",
        email
    )
    .execute(&mut *conn)
    .await?;

    sqlx::query!(
        "INSERT INTO workspace_member (workspace_id, email, role) VALUES ('default', $1, 'admin')
         ON CONFLICT (workspace_id, email) DO NOTHING",
        email
    )
    .execute(&mut *conn)
    .await?;

    Ok(())
}

pub(crate) fn generate_access_token(email: &str) -> Result<String, ApiError> {
    let now = chrono::Utc::now();
    let claims = Claims {
        email: email.to_string(),
        exp: (now + chrono::Duration::seconds(ACCESS_TOKEN_TTL_SECS as i64)).timestamp() as u64,
        iat: now.timestamp() as u64,
    };
    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(JWT_SECRET.as_bytes()),
    )?;
    Ok(token)
}

async fn create_session(db: &PgPool, email: &str) -> Result<String, ApiError> {
    let raw_token = Uuid::new_v4().to_string();
    let token_hash = sha256_hex(&raw_token);
    let expires_at = chrono::Utc::now() + chrono::Duration::seconds(REFRESH_TOKEN_TTL_SECS);

    sqlx::query!(
        "INSERT INTO session (email, token_hash, expires_at) VALUES ($1, $2, $3)",
        email,
        token_hash,
        expires_at
    )
    .execute(db)
    .await?;

    Ok(raw_token)
}

fn build_refresh_cookie(token: &str, max_age_secs: i64) -> HeaderValue {
    let cookie = format!(
        "refresh_token={token}; HttpOnly; Secure; SameSite=Strict; Path=/api/auth; Max-Age={max_age_secs}"
    );
    // Cookie values are constructed from known-safe ASCII strings
    #[allow(clippy::expect_used)]
    HeaderValue::from_str(&cookie).expect("cookie value is valid ASCII")
}

fn build_clear_refresh_cookie() -> HeaderValue {
    HeaderValue::from_static(
        "refresh_token=; HttpOnly; Secure; SameSite=Strict; Path=/api/auth; Max-Age=0",
    )
}

fn extract_refresh_token(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get(COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|cookies| {
            cookies
                .split(';')
                .map(str::trim)
                .find_map(|c| c.strip_prefix("refresh_token=").map(String::from))
        })
}

#[derive(serde::Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(serde::Deserialize)]
pub struct SignupRequest {
    pub email: String,
    pub password: String,
}

#[derive(serde::Serialize)]
pub struct AuthResponse {
    pub access_token: String,
    pub expires_in: u64,
    pub email: String,
}

#[tracing::instrument(name = "api::login", skip(db, req))]
pub async fn login(
    State(db): State<PgPool>,
    Json(req): Json<LoginRequest>,
) -> Result<Response, ApiError> {
    let account = sqlx::query!(
        "SELECT email, password_hash FROM account WHERE email = $1",
        req.email
    )
    .fetch_optional(&db)
    .await?
    .ok_or(ApiError::Unauthorized)?;

    let parsed_hash = PasswordHash::new(&account.password_hash)
        .map_err(|_| ApiError::Internal("hash parse error".into()))?;
    Argon2::default()
        .verify_password(req.password.as_bytes(), &parsed_hash)
        .map_err(|_| ApiError::Unauthorized)?;

    ensure_default_workspace(&db, &account.email).await?;

    let access_token = generate_access_token(&account.email)?;
    let refresh_token_raw = create_session(&db, &account.email).await?;

    let body = AuthResponse {
        access_token,
        expires_in: ACCESS_TOKEN_TTL_SECS,
        email: account.email,
    };

    Ok((
        StatusCode::OK,
        [(
            SET_COOKIE,
            build_refresh_cookie(&refresh_token_raw, REFRESH_TOKEN_TTL_SECS),
        )],
        Json(body),
    )
        .into_response())
}

/// Hash a plaintext password with Argon2 (random salt) for storage.
pub(crate) fn hash_password(password: &str) -> Result<String, ApiError> {
    let salt = SaltString::generate(&mut argon2::password_hash::rand_core::OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| ApiError::Internal(format!("hash error: {e}")))
}

/// Create an instance-admin account for `email` with `password` and a
/// default-workspace membership, **only if the account does not already exist**.
///
/// Create-only is deliberate: a matching pre-existing account is left completely
/// untouched (no promotion, no password change). Otherwise this env-driven path
/// would be a privilege-escalation primitive — a typo'd or injected
/// `COVEFLOW_INSTANCE_ADMIN_EMAIL` pointing at an existing user's address would
/// silently make them a cluster-wide admin. Promote an existing account
/// explicitly via SQL instead. Safe to run on every startup.
pub async fn bootstrap_admin(db: &PgPool, email: &str, password: &str) -> Result<(), ApiError> {
    let password_hash = hash_password(password)?;

    // Single transaction: the account and its default-workspace membership must
    // commit together, otherwise a half-written admin would exist but fail login
    // with "not a member of this workspace".
    let mut tx = db.begin().await?;

    // ON CONFLICT DO NOTHING → RETURNING yields a row only when we actually
    // inserted; a conflict (existing account) returns None and touches nothing.
    let created = sqlx::query_scalar!(
        r#"INSERT INTO account (email, password_hash, is_admin)
           VALUES ($1, $2, TRUE)
           ON CONFLICT (email) DO NOTHING
           RETURNING email"#,
        email,
        password_hash,
    )
    .fetch_optional(&mut *tx)
    .await?
    .is_some();

    if created {
        ensure_default_workspace_membership(&mut tx, email).await?;
        tx.commit().await?;
        tracing::info!(admin_email = %email, "bootstrapped instance-admin account");
        return Ok(());
    }

    // Existing account: inspect is_admin so we report accurately and only self-heal
    // a genuine admin. Never promote a non-admin (privilege-escalation guard) and
    // never touch the password.
    let already_admin = sqlx::query_scalar!(
        r#"SELECT is_admin AS "is_admin!" FROM account WHERE email = $1"#,
        email,
    )
    .fetch_one(&mut *tx)
    .await?;

    if already_admin {
        // Self-heal: an admin missing its default-workspace membership (created via
        // raw SQL, or a partial older code path) couldn't otherwise log in. The
        // insert is idempotent and leaves is_admin/password alone.
        ensure_default_workspace_membership(&mut tx, email).await?;
        tx.commit().await?;
        tracing::info!(
            admin_email = %email,
            "instance-admin account already exists; ensured default-workspace membership"
        );
    } else {
        tx.commit().await?;
        tracing::warn!(
            admin_email = %email,
            "account exists as a non-admin; not promoting and password not applied \
             — grant instance-admin explicitly via SQL if intended"
        );
    }
    Ok(())
}

#[tracing::instrument(name = "api::signup", skip(db, req))]
pub async fn signup(
    State(db): State<PgPool>,
    Json(req): Json<SignupRequest>,
) -> Result<Response, ApiError> {
    if req.password.len() < 8 {
        return Err(ApiError::BadRequest(
            "password must be at least 8 characters".into(),
        ));
    }

    let password_hash = hash_password(&req.password)?;

    // Account + default-workspace membership commit together: a half-written
    // signup (account but no membership) would log in to "not a member".
    let mut tx = db.begin().await?;
    sqlx::query!(
        "INSERT INTO account (email, password_hash) VALUES ($1, $2)",
        req.email,
        password_hash
    )
    .execute(&mut *tx)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(ref db_err) if db_err.is_unique_violation() => {
            ApiError::Conflict("email already registered".into())
        }
        other => ApiError::Db(other),
    })?;
    ensure_default_workspace_membership(&mut tx, &req.email).await?;
    tx.commit().await?;

    let access_token = generate_access_token(&req.email)?;
    let refresh_token_raw = create_session(&db, &req.email).await?;

    let body = AuthResponse {
        access_token,
        expires_in: ACCESS_TOKEN_TTL_SECS,
        email: req.email,
    };

    Ok((
        StatusCode::OK,
        [(
            SET_COOKIE,
            build_refresh_cookie(&refresh_token_raw, REFRESH_TOKEN_TTL_SECS),
        )],
        Json(body),
    )
        .into_response())
}

#[tracing::instrument(name = "api::refresh", skip(db, req))]
pub async fn refresh(
    State(db): State<PgPool>,
    req: Request<axum::body::Body>,
) -> Result<Response, ApiError> {
    let raw_token = extract_refresh_token(req.headers()).ok_or(ApiError::Unauthorized)?;
    let token_hash = sha256_hex(&raw_token);

    // Look up active, non-expired session
    let session = sqlx::query!(
        r#"SELECT id, email
           FROM session
           WHERE token_hash = $1
             AND revoked_at IS NULL
             AND expires_at > now()"#,
        token_hash
    )
    .fetch_optional(&db)
    .await?
    .ok_or(ApiError::Unauthorized)?;

    // Revoke old refresh token
    sqlx::query!(
        "UPDATE session SET revoked_at = now() WHERE id = $1",
        session.id
    )
    .execute(&db)
    .await?;

    // Issue new tokens
    let access_token = generate_access_token(&session.email)?;
    let new_refresh_token = create_session(&db, &session.email).await?;

    let body = AuthResponse {
        access_token,
        expires_in: ACCESS_TOKEN_TTL_SECS,
        email: session.email,
    };

    Ok((
        StatusCode::OK,
        [(
            SET_COOKIE,
            build_refresh_cookie(&new_refresh_token, REFRESH_TOKEN_TTL_SECS),
        )],
        Json(body),
    )
        .into_response())
}

#[tracing::instrument(name = "api::logout", skip(db, req))]
pub async fn logout(
    State(db): State<PgPool>,
    req: Request<axum::body::Body>,
) -> Result<Response, ApiError> {
    if let Some(raw_token) = extract_refresh_token(req.headers()) {
        let token_hash = sha256_hex(&raw_token);
        sqlx::query!(
            "UPDATE session SET revoked_at = now() WHERE token_hash = $1 AND revoked_at IS NULL",
            token_hash
        )
        .execute(&db)
        .await?;
    }

    Ok((
        StatusCode::OK,
        [(SET_COOKIE, build_clear_refresh_cookie())],
        Json(serde_json::json!({ "message": "logged out" })),
    )
        .into_response())
}

/// Revoke all active sessions for a user (use on password change, etc.)
pub async fn revoke_all_sessions(db: &PgPool, email: &str) -> Result<u64, ApiError> {
    let result = sqlx::query!(
        "UPDATE session SET revoked_at = now() WHERE email = $1 AND revoked_at IS NULL",
        email
    )
    .execute(db)
    .await?;

    Ok(result.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    // alice@example.com, given a role and a set of team roles.
    fn user(role: WorkspaceRole, team_roles: &[(&str, FolderRole)]) -> AuthedUser {
        AuthedUser {
            email: "alice@example.com".to_string(),
            workspace_id: "ws1".to_string(),
            role,
            teams: team_roles.iter().map(|(t, _)| t.to_string()).collect(),
            team_roles: team_roles
                .iter()
                .map(|(t, r)| (t.to_string(), *r))
                .collect(),
        }
    }

    #[test]
    fn test_admin_bypasses_all() {
        let u = user(WorkspaceRole::Admin, &[]);
        assert!(u.can_read("users/bob@example.com/x.py"));
        assert!(u.can_write("teams/anything/x.py"));
        assert!(u.can_write("workspace/x.py"));
        assert!(u.can_write("legacy/x.py")); // god-mode reads/writes non-root too
    }

    #[test]
    fn test_personal_path() {
        let u = user(WorkspaceRole::Editor, &[]);
        assert!(u.can_read("users/alice@example.com/script.py"));
        assert!(u.can_write("users/alice@example.com/script.py"));
        assert!(!u.can_read("users/bob@example.com/script.py"));
        assert!(!u.can_write("users/bob@example.com/script.py"));
    }

    #[test]
    fn test_team_reader_and_writer() {
        let reader = user(WorkspaceRole::Editor, &[("ml", FolderRole::Reader)]);
        assert!(reader.can_read("teams/ml/train.py"));
        assert!(!reader.can_write("teams/ml/train.py"));

        let writer = user(WorkspaceRole::Editor, &[("ml", FolderRole::Writer)]);
        assert!(writer.can_read("teams/ml/train.py"));
        assert!(writer.can_write("teams/ml/train.py"));

        // Not a member of "other".
        assert!(!writer.can_read("teams/other/x.py"));
        assert!(!writer.can_write("teams/other/x.py"));
    }

    #[test]
    fn test_workspace_share_honors_role() {
        let editor = user(WorkspaceRole::Editor, &[]);
        assert!(editor.can_read("workspace/x.py"));
        assert!(editor.can_write("workspace/x.py"));

        let viewer = user(WorkspaceRole::Viewer, &[]);
        assert!(viewer.can_read("workspace/x.py")); // read
        assert!(!viewer.can_write("workspace/x.py")); // but not write
    }

    #[test]
    fn test_non_root_path_denied_for_non_admin() {
        let u = user(WorkspaceRole::Editor, &[("ml", FolderRole::Writer)]);
        assert!(!u.can_read("a/x.py"));
        assert!(!u.can_write("f/etl/x.py"));
        assert!(!u.can_read("folders/secret/x.py")); // old model gone
    }

    #[test]
    fn test_is_valid_root_path() {
        let u = user(WorkspaceRole::Editor, &[("ml", FolderRole::Writer)]);
        assert!(u.is_valid_root_path("users/alice@example.com/x.py"));
        assert!(u.is_valid_root_path("teams/ml/x.py"));
        assert!(u.is_valid_root_path("workspace/x.py"));
        // Wrong owner / not-a-member / bare root / non-conforming → rejected.
        assert!(!u.is_valid_root_path("users/bob@example.com/x.py"));
        assert!(!u.is_valid_root_path("teams/other/x.py"));
        assert!(!u.is_valid_root_path("workspace")); // no file under it
        assert!(!u.is_valid_root_path("a/x.py"));
        // Admin may target any existing root shape, but still not a 4th root.
        let admin = user(WorkspaceRole::Admin, &[]);
        assert!(admin.is_valid_root_path("teams/anything/x.py"));
        assert!(!admin.is_valid_root_path("a/x.py"));
    }

    #[test]
    fn test_folder_role_ordering() {
        assert!(FolderRole::Owner > FolderRole::Writer);
        assert!(FolderRole::Writer > FolderRole::Reader);
    }

    #[test]
    fn test_require_writer_error() {
        let user = user(WorkspaceRole::Editor, &[]);
        let err = user.require_writer("folders/secret/file.py").unwrap_err();
        assert!(matches!(err, ApiError::Forbidden(_)));
    }

    #[test]
    fn test_sha256_hex() {
        let hash = sha256_hex("test-token");
        assert_eq!(hash.len(), 64);
        // SHA-256 of "test-token" is deterministic
        let hash2 = sha256_hex("test-token");
        assert_eq!(hash, hash2);
        // Different input produces different hash
        let hash3 = sha256_hex("other-token");
        assert_ne!(hash, hash3);
    }

    #[test]
    fn test_generate_access_token() {
        let token = generate_access_token("alice@example.com");
        assert!(token.is_ok());
        let token = token.unwrap();
        // JWT has 3 parts separated by dots
        assert_eq!(token.split('.').count(), 3);
    }

    #[test]
    fn test_access_token_claims() {
        let token = generate_access_token("alice@example.com").unwrap();
        let decoded = decode::<Claims>(
            &token,
            &DecodingKey::from_secret(JWT_SECRET.as_bytes()),
            &Validation::default(),
        )
        .unwrap();
        assert_eq!(decoded.claims.email, "alice@example.com");
        assert!(decoded.claims.iat > 0);
        // exp should be ~15 minutes from iat
        let diff = decoded.claims.exp - decoded.claims.iat;
        assert_eq!(diff, ACCESS_TOKEN_TTL_SECS);
    }

    #[test]
    fn test_build_refresh_cookie() {
        let cookie = build_refresh_cookie("test-uuid", 604_800);
        let s = cookie.to_str().unwrap();
        assert!(s.contains("refresh_token=test-uuid"));
        assert!(s.contains("HttpOnly"));
        assert!(s.contains("Secure"));
        assert!(s.contains("SameSite=Strict"));
        assert!(s.contains("Path=/api/auth"));
        assert!(s.contains("Max-Age=604800"));
    }

    #[test]
    fn test_build_clear_refresh_cookie() {
        let cookie = build_clear_refresh_cookie();
        let s = cookie.to_str().unwrap();
        assert!(s.contains("refresh_token=;"));
        assert!(s.contains("Max-Age=0"));
    }

    #[test]
    fn test_extract_refresh_token() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            COOKIE,
            HeaderValue::from_static("refresh_token=abc-123; other=value"),
        );
        assert_eq!(extract_refresh_token(&headers), Some("abc-123".to_string()));
    }

    #[test]
    fn test_extract_refresh_token_missing() {
        let headers = axum::http::HeaderMap::new();
        assert_eq!(extract_refresh_token(&headers), None);
    }

    #[test]
    fn test_extract_refresh_token_no_match() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(COOKIE, HeaderValue::from_static("other=value"));
        assert_eq!(extract_refresh_token(&headers), None);
    }

    #[test]
    fn test_workspace_role_from_db_valid() {
        assert_eq!(
            WorkspaceRole::from_db("admin").unwrap(),
            WorkspaceRole::Admin
        );
        assert_eq!(
            WorkspaceRole::from_db("editor").unwrap(),
            WorkspaceRole::Editor
        );
        assert_eq!(
            WorkspaceRole::from_db("viewer").unwrap(),
            WorkspaceRole::Viewer
        );
        assert_eq!(
            WorkspaceRole::from_db("operator").unwrap(),
            WorkspaceRole::Operator
        );
    }

    #[test]
    fn test_workspace_role_from_db_invalid() {
        let err = WorkspaceRole::from_db("superadmin");
        assert!(err.is_err());
    }

    #[test]
    fn test_workspace_role_serializes_lowercase() {
        assert_eq!(
            serde_json::to_string(&WorkspaceRole::Admin).unwrap(),
            r#""admin""#
        );
        assert_eq!(
            serde_json::to_string(&WorkspaceRole::Operator).unwrap(),
            r#""operator""#
        );
    }

    #[test]
    fn test_workspace_role_is_admin() {
        assert!(WorkspaceRole::Admin.is_admin());
        assert!(!WorkspaceRole::Editor.is_admin());
        assert!(!WorkspaceRole::Viewer.is_admin());
        assert!(!WorkspaceRole::Operator.is_admin());
    }

    #[test]
    fn test_auth_response_includes_email() {
        let resp = AuthResponse {
            access_token: "tok".to_string(),
            expires_in: 900,
            email: "alice@example.com".to_string(),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["email"], "alice@example.com");
        assert!(json.get("access_token").is_some());
        assert!(json.get("expires_in").is_some());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn bootstrap_admin_creates_and_is_idempotent(pool: PgPool) {
        bootstrap_admin(&pool, "root@test.local", "supersecret")
            .await
            .unwrap();

        let is_admin = sqlx::query_scalar!(
            r#"SELECT is_admin AS "is_admin!" FROM account WHERE email = $1"#,
            "root@test.local"
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(is_admin);

        let role = sqlx::query_scalar!(
            "SELECT role FROM workspace_member WHERE workspace_id = 'default' AND email = $1",
            "root@test.local"
        )
        .fetch_optional(&pool)
        .await
        .unwrap();
        assert_eq!(role.as_deref(), Some("admin"));

        // Re-running (e.g. on restart) must not error or change anything.
        bootstrap_admin(&pool, "root@test.local", "supersecret")
            .await
            .unwrap();
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn bootstrap_admin_leaves_existing_account_untouched(pool: PgPool) {
        // Create-only: a pre-existing account must NOT be silently promoted or
        // have its password changed via the env var (privilege-escalation guard).
        let original = hash_password("originalpw").unwrap();
        sqlx::query!(
            "INSERT INTO account (email, password_hash, is_admin) VALUES ($1, $2, FALSE)",
            "existing@test.local",
            original
        )
        .execute(&pool)
        .await
        .unwrap();

        bootstrap_admin(&pool, "existing@test.local", "different-bootstrap-pw")
            .await
            .unwrap();

        let row = sqlx::query!(
            r#"SELECT is_admin AS "is_admin!", password_hash FROM account WHERE email = $1"#,
            "existing@test.local"
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(!row.is_admin, "existing account must NOT be auto-promoted");
        assert_eq!(
            row.password_hash, original,
            "existing password must not be overwritten"
        );
    }
}
