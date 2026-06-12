-- Cron scheduling. A `schedule` triggers a flow on a cron expression.
-- Flows are referenced by path (no stable flow_id); referential integrity is
-- maintained by move_flow (rewrites flow_path) and delete_flow (409 unless force).
CREATE TABLE schedule (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id    VARCHAR(50)  NOT NULL REFERENCES workspace(id),
    name            VARCHAR(255) NOT NULL,
    flow_path       VARCHAR(255) NOT NULL,
    cron_expr       VARCHAR(120) NOT NULL,
    timezone        VARCHAR(64)  NOT NULL DEFAULT 'UTC',
    args            JSONB        NOT NULL DEFAULT '{}',
    enabled         BOOLEAN      NOT NULL DEFAULT TRUE,
    catchup         BOOLEAN      NOT NULL DEFAULT FALSE,
    -- NULL = unlimited concurrent runs; 1 = no overlap; N = at most N.
    max_active_runs INTEGER,
    next_trigger_at TIMESTAMPTZ,
    last_triggered_at TIMESTAMPTZ,
    last_error      TEXT,
    created_by      VARCHAR(255) NOT NULL,
    created_at      TIMESTAMPTZ  NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ  NOT NULL DEFAULT now(),
    CONSTRAINT schedule_max_active_runs_positive
        CHECK (max_active_runs IS NULL OR max_active_runs > 0),
    UNIQUE (workspace_id, flow_path, name)
);

-- Scheduler loop hot path: enabled schedules that are due.
CREATE INDEX idx_schedule_due ON schedule (next_trigger_at) WHERE enabled = TRUE;
-- flow move/delete integrity lookups.
CREATE INDEX idx_schedule_flow ON schedule (workspace_id, flow_path);

-- Runs created by a schedule point back to it: powers max_active_runs counting
-- and per-schedule run history. ON DELETE SET NULL keeps historical runs.
ALTER TABLE run ADD COLUMN schedule_id UUID REFERENCES schedule(id) ON DELETE SET NULL;
CREATE INDEX idx_run_schedule ON run (schedule_id) WHERE schedule_id IS NOT NULL;
