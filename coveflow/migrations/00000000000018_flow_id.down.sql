-- Reverse: schedules reference the flow by path again.
ALTER TABLE schedule ADD COLUMN flow_path VARCHAR(255);

UPDATE schedule s
SET flow_path = (
    SELECT f.path FROM flow f
    WHERE f.workspace_id = s.workspace_id AND f.flow_id = s.flow_id
    ORDER BY f.revision DESC
    LIMIT 1
);

DELETE FROM schedule WHERE flow_path IS NULL;

ALTER TABLE schedule ALTER COLUMN flow_path SET NOT NULL;

ALTER TABLE schedule DROP CONSTRAINT schedule_workspace_id_flow_id_name_key;
DROP INDEX idx_schedule_flow;
ALTER TABLE schedule DROP COLUMN flow_id;
ALTER TABLE schedule ADD CONSTRAINT schedule_workspace_id_flow_path_name_key
    UNIQUE (workspace_id, flow_path, name);
CREATE INDEX idx_schedule_flow ON schedule (workspace_id, flow_path);

DROP INDEX IF EXISTS flow_workspace_flow_id_idx;
ALTER TABLE flow DROP COLUMN flow_id;
