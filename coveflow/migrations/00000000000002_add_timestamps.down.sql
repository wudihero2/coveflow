-- Revert: remove created_at / updated_at columns and triggers

-- ==========================================================================
-- 1. Drop all triggers
-- ==========================================================================

DROP TRIGGER IF EXISTS trg_workspace_updated_at ON workspace;
DROP TRIGGER IF EXISTS trg_account_updated_at ON account;
DROP TRIGGER IF EXISTS trg_workspace_member_updated_at ON workspace_member;
DROP TRIGGER IF EXISTS trg_session_updated_at ON session;
DROP TRIGGER IF EXISTS trg_team_updated_at ON team;
DROP TRIGGER IF EXISTS trg_team_member_updated_at ON team_member;
DROP TRIGGER IF EXISTS trg_team_quota_updated_at ON team_quota;
DROP TRIGGER IF EXISTS trg_team_acl_updated_at ON team_acl;
DROP TRIGGER IF EXISTS trg_folder_updated_at ON folder;
DROP TRIGGER IF EXISTS trg_folder_acl_updated_at ON folder_acl;
DROP TRIGGER IF EXISTS trg_script_updated_at ON script;
DROP TRIGGER IF EXISTS trg_flow_updated_at ON flow;
DROP TRIGGER IF EXISTS trg_flow_file_updated_at ON flow_file;
DROP TRIGGER IF EXISTS trg_run_updated_at ON run;
DROP TRIGGER IF EXISTS trg_run_queue_updated_at ON run_queue;
DROP TRIGGER IF EXISTS trg_run_completed_updated_at ON run_completed;
DROP TRIGGER IF EXISTS trg_run_log_updated_at ON run_log;
DROP TRIGGER IF EXISTS trg_run_flow_status_updated_at ON run_flow_status;
DROP TRIGGER IF EXISTS trg_run_state_history_updated_at ON run_state_history;
DROP TRIGGER IF EXISTS trg_event_log_updated_at ON event_log;
DROP TRIGGER IF EXISTS trg_concurrency_limit_updated_at ON concurrency_limit;
DROP TRIGGER IF EXISTS trg_resource_type_updated_at ON resource_type;
DROP TRIGGER IF EXISTS trg_resource_updated_at ON resource;
DROP TRIGGER IF EXISTS trg_variable_updated_at ON variable;
DROP TRIGGER IF EXISTS trg_approval_policy_updated_at ON approval_policy;
DROP TRIGGER IF EXISTS trg_deploy_request_updated_at ON deploy_request;
DROP TRIGGER IF EXISTS trg_deploy_approval_updated_at ON deploy_approval;
DROP TRIGGER IF EXISTS trg_worker_config_updated_at ON worker_config;
DROP TRIGGER IF EXISTS trg_worker_ping_updated_at ON worker_ping;
DROP TRIGGER IF EXISTS trg_workspace_settings_updated_at ON workspace_settings;

-- ==========================================================================
-- 2. Drop added updated_at columns
-- ==========================================================================

ALTER TABLE workspace          DROP COLUMN IF EXISTS updated_at;
ALTER TABLE account            DROP COLUMN IF EXISTS updated_at;
ALTER TABLE workspace_member   DROP COLUMN IF EXISTS updated_at;
ALTER TABLE session            DROP COLUMN IF EXISTS updated_at;
ALTER TABLE team               DROP COLUMN IF EXISTS updated_at;
ALTER TABLE team_member         DROP COLUMN IF EXISTS updated_at;
ALTER TABLE team_quota          DROP COLUMN IF EXISTS updated_at;
ALTER TABLE team_acl            DROP COLUMN IF EXISTS updated_at;
ALTER TABLE folder              DROP COLUMN IF EXISTS updated_at;
ALTER TABLE folder_acl          DROP COLUMN IF EXISTS updated_at;
ALTER TABLE script             DROP COLUMN IF EXISTS updated_at;
ALTER TABLE flow               DROP COLUMN IF EXISTS updated_at;
-- flow_file.updated_at existed before this migration, do NOT drop it
ALTER TABLE run                DROP COLUMN IF EXISTS updated_at;
ALTER TABLE run_queue          DROP COLUMN IF EXISTS updated_at;
ALTER TABLE run_completed      DROP COLUMN IF EXISTS updated_at;
ALTER TABLE run_log            DROP COLUMN IF EXISTS updated_at;
ALTER TABLE run_flow_status    DROP COLUMN IF EXISTS updated_at;
ALTER TABLE run_state_history  DROP COLUMN IF EXISTS updated_at;
ALTER TABLE event_log          DROP COLUMN IF EXISTS updated_at;
ALTER TABLE concurrency_limit  DROP COLUMN IF EXISTS updated_at;
ALTER TABLE resource_type      DROP COLUMN IF EXISTS updated_at;
ALTER TABLE resource           DROP COLUMN IF EXISTS updated_at;
ALTER TABLE variable           DROP COLUMN IF EXISTS updated_at;
ALTER TABLE approval_policy    DROP COLUMN IF EXISTS updated_at;
ALTER TABLE deploy_request     DROP COLUMN IF EXISTS updated_at;
ALTER TABLE deploy_approval    DROP COLUMN IF EXISTS updated_at;
ALTER TABLE worker_config      DROP COLUMN IF EXISTS updated_at;
ALTER TABLE worker_ping        DROP COLUMN IF EXISTS updated_at;
ALTER TABLE workspace_settings DROP COLUMN IF EXISTS updated_at;

-- ==========================================================================
-- 3. Drop added created_at columns
-- ==========================================================================

ALTER TABLE workspace_member   DROP COLUMN IF EXISTS created_at;
ALTER TABLE team               DROP COLUMN IF EXISTS created_at;
ALTER TABLE team_member         DROP COLUMN IF EXISTS created_at;
ALTER TABLE team_quota          DROP COLUMN IF EXISTS created_at;
ALTER TABLE team_acl            DROP COLUMN IF EXISTS created_at;
ALTER TABLE folder              DROP COLUMN IF EXISTS created_at;
ALTER TABLE folder_acl          DROP COLUMN IF EXISTS created_at;
ALTER TABLE flow_file           DROP COLUMN IF EXISTS created_at;
ALTER TABLE run_queue           DROP COLUMN IF EXISTS created_at;
ALTER TABLE run_log             DROP COLUMN IF EXISTS created_at;
ALTER TABLE run_flow_status     DROP COLUMN IF EXISTS created_at;
ALTER TABLE concurrency_limit   DROP COLUMN IF EXISTS created_at;
ALTER TABLE resource_type       DROP COLUMN IF EXISTS created_at;
ALTER TABLE variable            DROP COLUMN IF EXISTS created_at;
ALTER TABLE approval_policy     DROP COLUMN IF EXISTS created_at;
ALTER TABLE worker_config       DROP COLUMN IF EXISTS created_at;
ALTER TABLE worker_ping         DROP COLUMN IF EXISTS created_at;
ALTER TABLE workspace_settings  DROP COLUMN IF EXISTS created_at;

-- ==========================================================================
-- 4. Drop trigger function
-- ==========================================================================

DROP FUNCTION IF EXISTS set_updated_at();
