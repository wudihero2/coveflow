-- Generic trigger framework. A `trigger` fires a flow run via a
-- type-specific path (v1: webhook = inbound HTTP). Cron schedules stay in their
-- own `schedule` table (different polling model); both converge on submit_run.
-- Flows are referenced by stable `flow_id` (path resolved on demand, like schedule).
CREATE TABLE trigger (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id  VARCHAR(50)  NOT NULL REFERENCES workspace(id),
    flow_id       UUID         NOT NULL,
    trigger_type  VARCHAR(32)  NOT NULL,          -- 'webhook' (future: flow_complete, file, ...)
    name          VARCHAR(255) NOT NULL,
    enabled       BOOLEAN      NOT NULL DEFAULT TRUE,
    config        JSONB        NOT NULL DEFAULT '{}',  -- type-specific (webhook: max_active_runs?)
    created_by    VARCHAR(255) NOT NULL,
    created_at    TIMESTAMPTZ  NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ  NOT NULL DEFAULT now(),
    UNIQUE (workspace_id, flow_id, name)
);

CREATE INDEX idx_trigger_flow ON trigger (workspace_id, flow_id);

-- Which trigger fired a run (parallels the existing schedule_id). Powers per-trigger
-- history + max_active_runs counting. ON DELETE SET NULL keeps historical runs.
ALTER TABLE run ADD COLUMN trigger_id UUID REFERENCES trigger(id) ON DELETE SET NULL;
CREATE INDEX idx_run_trigger ON run (trigger_id) WHERE trigger_id IS NOT NULL;

-- Trigger provenance (webhook: method / source_ip / header summary / time). Surfaced
-- into the run context as `ctx.trigger` (build_run_context). NULL for non-trigger runs.
ALTER TABLE run ADD COLUMN trigger_context JSONB;
