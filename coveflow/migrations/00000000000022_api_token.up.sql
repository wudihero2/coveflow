-- Personal API Token (PAT): a user-owned bearer credential. v1 is used
-- only to authenticate inbound webhook calls (the run executes as the token's
-- owner). Account-global (one owner, valid across workspaces; what it can do is
-- bounded by the owner's real per-workspace permissions, checked at use).
--
-- The full token is shown once at creation and stored two ways: `token_hash`
-- (sha256, the unique O(1) auth lookup key) and `token_encrypted` (AES-256-GCM,
-- so the UI can reveal it again). Plaintext never touches the DB.
CREATE TABLE api_token (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    email           VARCHAR(255) NOT NULL REFERENCES account(email),
    name            VARCHAR(255) NOT NULL,
    token_hash      VARCHAR(64)  NOT NULL UNIQUE,
    token_encrypted BYTEA        NOT NULL,
    expires_at      TIMESTAMPTZ,                  -- NULL = never expires
    last_used_at    TIMESTAMPTZ,
    created_at      TIMESTAMPTZ  NOT NULL DEFAULT now(),
    UNIQUE (email, name)
);

CREATE INDEX idx_api_token_email ON api_token (email);
