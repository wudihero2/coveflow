DROP INDEX IF EXISTS idx_service_log_created_at;
DROP INDEX IF EXISTS idx_run_log_created_at;

ALTER TABLE team_quota DROP COLUMN IF EXISTS log_retention_days;
ALTER TABLE workspace_settings DROP COLUMN IF EXISTS log_retention_days;

ALTER TABLE run DROP CONSTRAINT run_kind_check;
ALTER TABLE run ADD CONSTRAINT run_kind_check
    CHECK (kind IN ('script', 'flow', 'preview', 'flow_preview'));
