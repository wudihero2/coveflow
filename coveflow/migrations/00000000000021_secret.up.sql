-- Secret store: workspace-scoped, path-addressed, write-only encrypted
-- key-value. `value_encrypted` is AES-256-GCM (`nonce ‖ ciphertext+tag`); plaintext
-- never touches the DB. Path uses the three-system-root model (users/<email>/..,
-- teams/<team>/.., workspace/..) so the existing ACL governs who can manage/read.
CREATE TABLE secret (
    workspace_id    VARCHAR(50)  NOT NULL REFERENCES workspace(id),
    path            VARCHAR(255) NOT NULL,
    value_encrypted BYTEA        NOT NULL,
    description     TEXT         NOT NULL DEFAULT '',
    created_by      VARCHAR(255) NOT NULL,
    updated_by      VARCHAR(255) NOT NULL,
    created_at      TIMESTAMPTZ  NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ  NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, path)
);

-- Worker injection hot path: fetch every secret in a workspace, then filter by
-- can_read in process (membership isn't expressible in SQL alone here).
CREATE INDEX idx_secret_workspace ON secret (workspace_id);
