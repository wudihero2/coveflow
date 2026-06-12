-- CoveFlow initial schema (consolidated from incremental migrations)

-- ==========================================================================
-- 1. Core: workspace, account, session
-- ==========================================================================

CREATE TABLE workspace (
    id         VARCHAR(50)  PRIMARY KEY,
    name       VARCHAR(255) NOT NULL,
    owner      VARCHAR(255) NOT NULL,
    created_at TIMESTAMPTZ  NOT NULL DEFAULT now()
);

CREATE TABLE account (
    email         VARCHAR(255) PRIMARY KEY,
    password_hash VARCHAR(255) NOT NULL,  -- argon2
    is_admin      BOOLEAN      NOT NULL DEFAULT FALSE,
    created_at    TIMESTAMPTZ  NOT NULL DEFAULT now()
);

CREATE TABLE workspace_member (
    workspace_id VARCHAR(50)  NOT NULL REFERENCES workspace(id),
    email        VARCHAR(255) NOT NULL REFERENCES account(email),
    role         VARCHAR(20)  NOT NULL DEFAULT 'editor'
                 CHECK (role IN ('admin', 'editor', 'viewer', 'operator')),
    PRIMARY KEY (workspace_id, email)
);

CREATE TABLE session (
    id          UUID         PRIMARY KEY DEFAULT gen_random_uuid(),
    email       VARCHAR(255) NOT NULL REFERENCES account(email) ON DELETE CASCADE,
    token_hash  VARCHAR(64)  NOT NULL UNIQUE,  -- SHA-256 hex of refresh token
    expires_at  TIMESTAMPTZ  NOT NULL,
    created_at  TIMESTAMPTZ  NOT NULL DEFAULT now(),
    revoked_at  TIMESTAMPTZ  DEFAULT NULL       -- NULL = active
);

CREATE INDEX idx_session_email ON session(email);
CREATE INDEX idx_session_token_hash ON session(token_hash) WHERE revoked_at IS NULL;

-- ==========================================================================
-- 2. Teams & folders with normalized ACL
-- ==========================================================================

CREATE TABLE team (
    workspace_id VARCHAR(50)  NOT NULL REFERENCES workspace(id),
    name         VARCHAR(100) NOT NULL,
    summary      TEXT         DEFAULT '',
    PRIMARY KEY (workspace_id, name)
);

CREATE TABLE team_member (
    workspace_id VARCHAR(50)  NOT NULL,
    email        VARCHAR(255) NOT NULL,
    team_name    VARCHAR(100) NOT NULL,
    PRIMARY KEY (workspace_id, email, team_name),
    FOREIGN KEY (workspace_id, team_name)
        REFERENCES team(workspace_id, name) ON DELETE CASCADE
);

CREATE TABLE team_quota (
    workspace_id         VARCHAR(50)  NOT NULL,
    team_name            VARCHAR(100) NOT NULL,
    max_concurrent_runs  INTEGER,
    max_cpus             REAL,
    max_memory_mb        BIGINT,
    max_daily_runs       INTEGER,
    max_storage_bytes    BIGINT,
    max_run_timeout_secs INTEGER,
    PRIMARY KEY (workspace_id, team_name),
    FOREIGN KEY (workspace_id, team_name)
        REFERENCES team(workspace_id, name) ON DELETE CASCADE
);

CREATE TABLE team_acl (
    workspace_id VARCHAR(50)  NOT NULL,
    team_name    VARCHAR(100) NOT NULL,
    subject      VARCHAR(255) NOT NULL,
    role         VARCHAR(20)  NOT NULL CHECK (role IN ('manager')),
    PRIMARY KEY (workspace_id, team_name, subject),
    FOREIGN KEY (workspace_id, team_name)
        REFERENCES team(workspace_id, name) ON DELETE CASCADE
);

CREATE INDEX idx_team_acl_subject ON team_acl(workspace_id, subject);

CREATE TABLE folder (
    workspace_id VARCHAR(50)  NOT NULL REFERENCES workspace(id),
    name         VARCHAR(100) NOT NULL,
    display_name VARCHAR(255) DEFAULT '',
    PRIMARY KEY (workspace_id, name)
);

CREATE TABLE folder_acl (
    workspace_id VARCHAR(50)  NOT NULL,
    folder_name  VARCHAR(100) NOT NULL,
    subject      VARCHAR(255) NOT NULL,  -- 'users/alice' or 'teams/backend'
    role         VARCHAR(20)  NOT NULL CHECK (role IN ('owner', 'writer', 'reader')),
    PRIMARY KEY (workspace_id, folder_name, subject),
    FOREIGN KEY (workspace_id, folder_name)
        REFERENCES folder(workspace_id, name) ON DELETE CASCADE
);

CREATE INDEX idx_folder_acl_subject ON folder_acl(workspace_id, subject);

-- ==========================================================================
-- 3. Scripts & flows
-- ==========================================================================

CREATE TABLE script (
    workspace_id  VARCHAR(50)  NOT NULL REFERENCES workspace(id),
    hash          CHAR(64)     NOT NULL,            -- SHA256
    path          VARCHAR(255) NOT NULL,
    content       TEXT         NOT NULL,
    language      VARCHAR(20)  NOT NULL
                  CHECK (language IN ('python3', 'typescript', 'bash', 'duckdb')),
    schema        JSONB,
    parent_hashes TEXT[],
    summary       TEXT         DEFAULT '',
    requirements  TEXT[]       NOT NULL DEFAULT '{}',
    created_by    VARCHAR(255) NOT NULL,
    created_at    TIMESTAMPTZ  NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, hash)
);

CREATE INDEX idx_script_path ON script(workspace_id, path, created_at DESC);

CREATE TABLE flow (
    workspace_id VARCHAR(50)  NOT NULL REFERENCES workspace(id),
    path         VARCHAR(255) NOT NULL,
    revision     INTEGER      NOT NULL DEFAULT 1,
    summary      TEXT         DEFAULT '',
    description  TEXT         DEFAULT '',
    value        JSONB        NOT NULL,
    schema       JSONB,
    edited_by    VARCHAR(255) NOT NULL,
    edited_at    TIMESTAMPTZ  NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, path, revision)
);

CREATE INDEX idx_flow_latest ON flow(workspace_id, path, revision DESC);

CREATE TABLE flow_file (
    workspace_id VARCHAR(50)  NOT NULL,
    flow_path    VARCHAR(255) NOT NULL,
    file_path    VARCHAR(500) NOT NULL,
    content      TEXT         NOT NULL,
    updated_at   TIMESTAMPTZ  NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, flow_path, file_path)
);

-- ==========================================================================
-- 4. Run execution
-- ==========================================================================

CREATE TABLE run (
    id            UUID         PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id  VARCHAR(50)  NOT NULL REFERENCES workspace(id),
    kind          VARCHAR(20)  NOT NULL
                  CHECK (kind IN ('script', 'flow', 'preview', 'flow_preview')),
    script_hash   CHAR(64),
    script_path   VARCHAR(255),
    flow_value    JSONB,
    raw_code      TEXT,
    language      VARCHAR(20),
    args          JSONB,
    requirements  TEXT[]       NOT NULL DEFAULT '{}',
    tag           VARCHAR(50)  NOT NULL DEFAULT 'default',
    parent_run    UUID,
    root_run      UUID,
    flow_step_id  VARCHAR(50),
    flow_revision INTEGER,
    cpus          REAL         NOT NULL DEFAULT 1,
    memory_mb     INTEGER      NOT NULL DEFAULT 512,
    disk_mb       INTEGER      NOT NULL DEFAULT 1024,
    timeout       INTEGER,
    custom_image  VARCHAR(255),
    team_owner    VARCHAR(100),
    rerun_of      UUID         REFERENCES run(id),
    created_by    VARCHAR(255) NOT NULL,
    created_at    TIMESTAMPTZ  NOT NULL DEFAULT now(),
    trace_id      CHAR(32),
    span_id       CHAR(16)
);

CREATE TABLE run_queue (
    id                   UUID         PRIMARY KEY REFERENCES run(id),
    scheduled_for        TIMESTAMPTZ  NOT NULL DEFAULT now(),
    running              BOOLEAN      NOT NULL DEFAULT FALSE,
    started_at           TIMESTAMPTZ,
    tag                  VARCHAR(50)  NOT NULL DEFAULT 'default',
    priority             SMALLINT     NOT NULL DEFAULT 0,
    worker               VARCHAR(100),
    last_ping            TIMESTAMPTZ,
    canceled_by          VARCHAR(255),
    canceled_reason      TEXT,
    cancel_requested_at  TIMESTAMPTZ
);

CREATE INDEX idx_run_queue_pull ON run_queue(scheduled_for, priority DESC)
    WHERE running = FALSE;

CREATE INDEX idx_run_queue_cancel ON run_queue(id)
    WHERE canceled_by IS NOT NULL AND running = TRUE;

CREATE TABLE run_completed (
    id                UUID         PRIMARY KEY REFERENCES run(id),
    success           BOOLEAN      NOT NULL,
    result            JSONB,
    result_s3_key     VARCHAR(255),
    duration_ms       INTEGER      NOT NULL,
    memory_peak_bytes BIGINT,
    canceled_by       VARCHAR(255),
    canceled_reason   TEXT,
    marked_by         VARCHAR(255),
    mark_reason       TEXT,
    completed_at      TIMESTAMPTZ  NOT NULL DEFAULT now()
);

CREATE TABLE run_log (
    run_id UUID NOT NULL REFERENCES run(id),
    logs   TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (run_id)
);

CREATE TABLE run_flow_status (
    run_id      UUID  PRIMARY KEY REFERENCES run(id),
    flow_status JSONB NOT NULL
);

CREATE TABLE run_state_history (
    id        BIGSERIAL    PRIMARY KEY,
    run_id    UUID         NOT NULL REFERENCES run(id),
    state     VARCHAR(20)  NOT NULL
              CHECK (state IN ('queued', 'running', 'success', 'failure', 'cancelled', 'retrying')),
    timestamp TIMESTAMPTZ  NOT NULL DEFAULT now(),
    message   TEXT
);

CREATE INDEX idx_run_state_run ON run_state_history(run_id, timestamp);

-- ==========================================================================
-- 5. Events & concurrency
-- ==========================================================================

CREATE TABLE event_log (
    id           BIGSERIAL    PRIMARY KEY,
    workspace_id VARCHAR(50)  NOT NULL,
    event_type   VARCHAR(50)  NOT NULL,  -- run.created, run.completed, flow.saved, script.created
    entity_type  VARCHAR(20)  NOT NULL
                 CHECK (entity_type IN ('run', 'flow', 'script', 'schedule')),
    entity_id    VARCHAR(255) NOT NULL,
    payload      JSONB,
    timestamp    TIMESTAMPTZ  NOT NULL DEFAULT now()
);

CREATE INDEX idx_event_log_entity ON event_log(workspace_id, entity_type, entity_id);
CREATE INDEX idx_event_log_time   ON event_log(workspace_id, timestamp DESC);

CREATE TABLE concurrency_limit (
    workspace_id   VARCHAR(50) NOT NULL REFERENCES workspace(id),
    tag            VARCHAR(50) NOT NULL,
    max_concurrent INTEGER     NOT NULL DEFAULT 1,
    PRIMARY KEY (workspace_id, tag)
);

-- ==========================================================================
-- 6. Resources & variables
-- ==========================================================================

CREATE TABLE resource_type (
    workspace_id VARCHAR(50)  NOT NULL REFERENCES workspace(id),
    name         VARCHAR(100) NOT NULL,
    schema       JSONB        NOT NULL,
    description  TEXT         DEFAULT '',
    PRIMARY KEY (workspace_id, name)
);

CREATE TABLE resource (
    workspace_id    VARCHAR(50)  NOT NULL REFERENCES workspace(id),
    path            VARCHAR(255) NOT NULL,
    resource_type   VARCHAR(100) NOT NULL,
    value_encrypted BYTEA        NOT NULL,  -- AES-256-GCM
    description     TEXT         DEFAULT '',
    created_by      VARCHAR(255) NOT NULL,
    created_at      TIMESTAMPTZ  NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, path)
);

CREATE TABLE variable (
    workspace_id    VARCHAR(50)  NOT NULL REFERENCES workspace(id),
    path            VARCHAR(255) NOT NULL,
    value_encrypted BYTEA        NOT NULL,  -- AES-256-GCM
    is_secret       BOOLEAN      NOT NULL DEFAULT TRUE,
    description     TEXT         DEFAULT '',
    created_by      VARCHAR(255) NOT NULL,
    PRIMARY KEY (workspace_id, path)
);

-- ==========================================================================
-- 7. Deployment approval
-- ==========================================================================

CREATE TABLE approval_policy (
    workspace_id  VARCHAR(50)  NOT NULL REFERENCES workspace(id),
    path_pattern  VARCHAR(255) NOT NULL,  -- glob: 'f/production/*', 'f/finance/*'
    min_approvals INTEGER      NOT NULL DEFAULT 1,
    approvers     TEXT[]       NOT NULL,  -- ['u/alice', 'g/sre-team']
    auto_deploy   BOOLEAN      DEFAULT FALSE,
    PRIMARY KEY (workspace_id, path_pattern)
);

CREATE TABLE deploy_request (
    id            UUID         PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id  VARCHAR(50)  NOT NULL REFERENCES workspace(id),
    target_path   VARCHAR(255) NOT NULL,
    target_kind   VARCHAR(10)  NOT NULL
                  CHECK (target_kind IN ('flow', 'script')),
    draft_value   JSONB        NOT NULL,
    previous_hash VARCHAR(64),
    requested_by  VARCHAR(255) NOT NULL,
    status        VARCHAR(20)  NOT NULL DEFAULT 'pending'
                  CHECK (status IN ('pending', 'approved', 'rejected', 'deployed')),
    created_at    TIMESTAMPTZ  NOT NULL DEFAULT now(),
    deployed_at   TIMESTAMPTZ
);

CREATE INDEX idx_deploy_request_ws_status ON deploy_request(workspace_id, status);

CREATE TABLE deploy_approval (
    deploy_request_id UUID         NOT NULL REFERENCES deploy_request(id),
    approver          VARCHAR(255) NOT NULL,
    decision          VARCHAR(10)  NOT NULL
                      CHECK (decision IN ('approved', 'rejected')),
    comment           TEXT         DEFAULT '',
    decided_at        TIMESTAMPTZ  NOT NULL DEFAULT now(),
    PRIMARY KEY (deploy_request_id, approver)
);

-- ==========================================================================
-- 8. Worker management
-- ==========================================================================

CREATE TABLE worker_config (
    workspace_id        VARCHAR(50) NOT NULL REFERENCES workspace(id),
    max_concurrent_runs INTEGER,
    max_workers_per_tag JSONB       DEFAULT '{}',
    PRIMARY KEY (workspace_id)
);

CREATE TABLE worker_ping (
    worker             VARCHAR(100) PRIMARY KEY,
    ping_at            TIMESTAMPTZ  NOT NULL DEFAULT now(),
    tags               TEXT[]       NOT NULL DEFAULT '{}',
    ip                 VARCHAR(45),
    sandbox_mode       VARCHAR(20),
    current_run_id     UUID,
    runs_completed     INTEGER      DEFAULT 0,
    -- Resource-based capacity
    total_cpus         REAL,
    used_cpus          REAL,
    total_memory_mb    BIGINT,
    used_memory_mb     BIGINT,
    total_disk_mb      BIGINT,
    used_disk_mb       BIGINT,
    -- Static hardware info (detected at startup)
    vcpus              INTEGER,
    memory_total       BIGINT,
    disk_total         BIGINT,
    -- Live usage metrics (updated per ping)
    cpu_usage_percent  REAL,
    memory_usage       BIGINT,
    disk_usage         BIGINT,
    -- Occupancy windows
    occupancy_15s      REAL,
    occupancy_5m       REAL,
    occupancy_30m      REAL
);

-- ==========================================================================
-- 9. Workspace settings
-- ==========================================================================

CREATE TABLE workspace_settings (
    workspace_id             VARCHAR(50) PRIMARY KEY REFERENCES workspace(id),
    file_storage_mode        VARCHAR(10) NOT NULL DEFAULT 'local'
                             CHECK (file_storage_mode IN ('local', 's3')),
    s3_bucket                VARCHAR(255),
    s3_region                VARCHAR(50),
    s3_endpoint              VARCHAR(255),
    s3_access_key_encrypted  BYTEA,
    s3_secret_key_encrypted  BYTEA,
    local_data_dir           VARCHAR(500) DEFAULT '/data/coveflow/files',
    max_file_size            BIGINT       DEFAULT 104857600  -- 100MB
);
