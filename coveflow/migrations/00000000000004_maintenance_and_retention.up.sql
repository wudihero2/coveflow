-- Allow 'maintenance' as a run kind
ALTER TABLE run DROP CONSTRAINT run_kind_check;
ALTER TABLE run ADD CONSTRAINT run_kind_check
    CHECK (kind IN ('script', 'flow', 'preview', 'flow_preview', 'maintenance'));

-- Per-workspace log retention policy (days). NULL = use global default (30).
ALTER TABLE workspace_settings ADD COLUMN log_retention_days INTEGER;
ALTER TABLE workspace_settings ADD CONSTRAINT workspace_settings_log_retention_days_range
    CHECK (log_retention_days IS NULL OR log_retention_days BETWEEN 0 AND 100000);

-- Per-team log retention override. NULL = follow workspace setting.
ALTER TABLE team_quota ADD COLUMN log_retention_days INTEGER;
ALTER TABLE team_quota ADD CONSTRAINT team_quota_log_retention_days_range
    CHECK (log_retention_days IS NULL OR log_retention_days BETWEEN 0 AND 100000);

-- Indexes for retention cleanup queries (delete by created_at range)
CREATE INDEX idx_run_log_created_at ON run_log (created_at);
CREATE INDEX idx_service_log_created_at ON service_log (created_at);
