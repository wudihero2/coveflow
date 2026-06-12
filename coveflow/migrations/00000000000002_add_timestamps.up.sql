-- Add created_at / updated_at to ALL tables, with auto-update trigger.

-- ==========================================================================
-- 1. Trigger function (shared by all tables)
-- ==========================================================================

CREATE OR REPLACE FUNCTION set_updated_at()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = now();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- ==========================================================================
-- 2. Add missing created_at columns
-- ==========================================================================

ALTER TABLE workspace_member   ADD COLUMN created_at TIMESTAMPTZ NOT NULL DEFAULT now();
ALTER TABLE team               ADD COLUMN created_at TIMESTAMPTZ NOT NULL DEFAULT now();
ALTER TABLE team_member         ADD COLUMN created_at TIMESTAMPTZ NOT NULL DEFAULT now();
ALTER TABLE team_quota          ADD COLUMN created_at TIMESTAMPTZ NOT NULL DEFAULT now();
ALTER TABLE team_acl            ADD COLUMN created_at TIMESTAMPTZ NOT NULL DEFAULT now();
ALTER TABLE folder              ADD COLUMN created_at TIMESTAMPTZ NOT NULL DEFAULT now();
ALTER TABLE folder_acl          ADD COLUMN created_at TIMESTAMPTZ NOT NULL DEFAULT now();
ALTER TABLE flow_file           ADD COLUMN created_at TIMESTAMPTZ NOT NULL DEFAULT now();
ALTER TABLE run_queue           ADD COLUMN created_at TIMESTAMPTZ NOT NULL DEFAULT now();
ALTER TABLE run_log             ADD COLUMN created_at TIMESTAMPTZ NOT NULL DEFAULT now();
ALTER TABLE run_flow_status     ADD COLUMN created_at TIMESTAMPTZ NOT NULL DEFAULT now();
ALTER TABLE concurrency_limit   ADD COLUMN created_at TIMESTAMPTZ NOT NULL DEFAULT now();
ALTER TABLE resource_type       ADD COLUMN created_at TIMESTAMPTZ NOT NULL DEFAULT now();
ALTER TABLE variable            ADD COLUMN created_at TIMESTAMPTZ NOT NULL DEFAULT now();
ALTER TABLE approval_policy     ADD COLUMN created_at TIMESTAMPTZ NOT NULL DEFAULT now();
ALTER TABLE worker_config       ADD COLUMN created_at TIMESTAMPTZ NOT NULL DEFAULT now();
ALTER TABLE worker_ping         ADD COLUMN created_at TIMESTAMPTZ NOT NULL DEFAULT now();
ALTER TABLE workspace_settings  ADD COLUMN created_at TIMESTAMPTZ NOT NULL DEFAULT now();

-- ==========================================================================
-- 3. Add missing updated_at columns
-- ==========================================================================

-- Core
ALTER TABLE workspace          ADD COLUMN updated_at TIMESTAMPTZ NOT NULL DEFAULT now();
ALTER TABLE account            ADD COLUMN updated_at TIMESTAMPTZ NOT NULL DEFAULT now();
ALTER TABLE workspace_member   ADD COLUMN updated_at TIMESTAMPTZ NOT NULL DEFAULT now();
ALTER TABLE session            ADD COLUMN updated_at TIMESTAMPTZ NOT NULL DEFAULT now();

-- Teams & folders
ALTER TABLE team               ADD COLUMN updated_at TIMESTAMPTZ NOT NULL DEFAULT now();
ALTER TABLE team_member         ADD COLUMN updated_at TIMESTAMPTZ NOT NULL DEFAULT now();
ALTER TABLE team_quota          ADD COLUMN updated_at TIMESTAMPTZ NOT NULL DEFAULT now();
ALTER TABLE team_acl            ADD COLUMN updated_at TIMESTAMPTZ NOT NULL DEFAULT now();
ALTER TABLE folder              ADD COLUMN updated_at TIMESTAMPTZ NOT NULL DEFAULT now();
ALTER TABLE folder_acl          ADD COLUMN updated_at TIMESTAMPTZ NOT NULL DEFAULT now();

-- Scripts & flows
ALTER TABLE script             ADD COLUMN updated_at TIMESTAMPTZ NOT NULL DEFAULT now();
ALTER TABLE flow               ADD COLUMN updated_at TIMESTAMPTZ NOT NULL DEFAULT now();
-- flow_file already has updated_at

-- Run execution
ALTER TABLE run                ADD COLUMN updated_at TIMESTAMPTZ NOT NULL DEFAULT now();
ALTER TABLE run_queue          ADD COLUMN updated_at TIMESTAMPTZ NOT NULL DEFAULT now();
ALTER TABLE run_completed      ADD COLUMN updated_at TIMESTAMPTZ NOT NULL DEFAULT now();
ALTER TABLE run_log            ADD COLUMN updated_at TIMESTAMPTZ NOT NULL DEFAULT now();
ALTER TABLE run_flow_status    ADD COLUMN updated_at TIMESTAMPTZ NOT NULL DEFAULT now();
ALTER TABLE run_state_history  ADD COLUMN updated_at TIMESTAMPTZ NOT NULL DEFAULT now();

-- Events & concurrency
ALTER TABLE event_log          ADD COLUMN updated_at TIMESTAMPTZ NOT NULL DEFAULT now();
ALTER TABLE concurrency_limit  ADD COLUMN updated_at TIMESTAMPTZ NOT NULL DEFAULT now();

-- Resources & variables
ALTER TABLE resource_type      ADD COLUMN updated_at TIMESTAMPTZ NOT NULL DEFAULT now();
ALTER TABLE resource           ADD COLUMN updated_at TIMESTAMPTZ NOT NULL DEFAULT now();
ALTER TABLE variable           ADD COLUMN updated_at TIMESTAMPTZ NOT NULL DEFAULT now();

-- Deployment approval
ALTER TABLE approval_policy    ADD COLUMN updated_at TIMESTAMPTZ NOT NULL DEFAULT now();
ALTER TABLE deploy_request     ADD COLUMN updated_at TIMESTAMPTZ NOT NULL DEFAULT now();
ALTER TABLE deploy_approval    ADD COLUMN updated_at TIMESTAMPTZ NOT NULL DEFAULT now();

-- Worker management
ALTER TABLE worker_config      ADD COLUMN updated_at TIMESTAMPTZ NOT NULL DEFAULT now();
ALTER TABLE worker_ping        ADD COLUMN updated_at TIMESTAMPTZ NOT NULL DEFAULT now();

-- Workspace settings
ALTER TABLE workspace_settings ADD COLUMN updated_at TIMESTAMPTZ NOT NULL DEFAULT now();

-- ==========================================================================
-- 4. Create triggers on ALL tables
-- ==========================================================================

CREATE TRIGGER trg_workspace_updated_at
    BEFORE UPDATE ON workspace FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER trg_account_updated_at
    BEFORE UPDATE ON account FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER trg_workspace_member_updated_at
    BEFORE UPDATE ON workspace_member FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER trg_session_updated_at
    BEFORE UPDATE ON session FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER trg_team_updated_at
    BEFORE UPDATE ON team FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER trg_team_member_updated_at
    BEFORE UPDATE ON team_member FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER trg_team_quota_updated_at
    BEFORE UPDATE ON team_quota FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER trg_team_acl_updated_at
    BEFORE UPDATE ON team_acl FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER trg_folder_updated_at
    BEFORE UPDATE ON folder FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER trg_folder_acl_updated_at
    BEFORE UPDATE ON folder_acl FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER trg_script_updated_at
    BEFORE UPDATE ON script FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER trg_flow_updated_at
    BEFORE UPDATE ON flow FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER trg_flow_file_updated_at
    BEFORE UPDATE ON flow_file FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER trg_run_updated_at
    BEFORE UPDATE ON run FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER trg_run_queue_updated_at
    BEFORE UPDATE ON run_queue FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER trg_run_completed_updated_at
    BEFORE UPDATE ON run_completed FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER trg_run_log_updated_at
    BEFORE UPDATE ON run_log FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER trg_run_flow_status_updated_at
    BEFORE UPDATE ON run_flow_status FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER trg_run_state_history_updated_at
    BEFORE UPDATE ON run_state_history FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER trg_event_log_updated_at
    BEFORE UPDATE ON event_log FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER trg_concurrency_limit_updated_at
    BEFORE UPDATE ON concurrency_limit FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER trg_resource_type_updated_at
    BEFORE UPDATE ON resource_type FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER trg_resource_updated_at
    BEFORE UPDATE ON resource FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER trg_variable_updated_at
    BEFORE UPDATE ON variable FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER trg_approval_policy_updated_at
    BEFORE UPDATE ON approval_policy FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER trg_deploy_request_updated_at
    BEFORE UPDATE ON deploy_request FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER trg_deploy_approval_updated_at
    BEFORE UPDATE ON deploy_approval FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER trg_worker_config_updated_at
    BEFORE UPDATE ON worker_config FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER trg_worker_ping_updated_at
    BEFORE UPDATE ON worker_ping FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER trg_workspace_settings_updated_at
    BEFORE UPDATE ON workspace_settings FOR EACH ROW EXECUTE FUNCTION set_updated_at();
