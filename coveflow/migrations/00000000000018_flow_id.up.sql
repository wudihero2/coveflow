-- Stable per-logical-flow identity, independent of path (which becomes a movable
-- label). All revisions of a flow that share (workspace_id, path) get the same
-- flow_id; schedules reference flow_id instead of path so that moving/renaming a
-- flow never breaks references. Mirrors script_id (migration 0012).
ALTER TABLE flow ADD COLUMN flow_id UUID;

-- Assign one id per logical flow (one per (workspace_id, path) group), applied to
-- every revision row of that path.
UPDATE flow f
SET flow_id = sub.id
FROM (
    SELECT workspace_id, path, gen_random_uuid() AS id
    FROM flow
    GROUP BY workspace_id, path
) sub
WHERE f.workspace_id = sub.workspace_id AND f.path = sub.path;

ALTER TABLE flow ALTER COLUMN flow_id SET NOT NULL;

-- Resolve "latest revision of this logical flow" by (workspace_id, flow_id).
CREATE INDEX flow_workspace_flow_id_idx ON flow (workspace_id, flow_id);

-- Schedules now reference the flow by stable id instead of path.
ALTER TABLE schedule ADD COLUMN flow_id UUID;

UPDATE schedule s
SET flow_id = (
    SELECT f.flow_id FROM flow f
    WHERE f.workspace_id = s.workspace_id AND f.path = s.flow_path
    LIMIT 1
);

-- Defensive: drop any schedule whose flow no longer exists (should be none given
-- the move/delete sync invariants that were in force) so SET NOT NULL cannot fail.
DELETE FROM schedule WHERE flow_id IS NULL;

ALTER TABLE schedule ALTER COLUMN flow_id SET NOT NULL;

-- Swap the path-based name uniqueness + lookup index for id-based ones.
ALTER TABLE schedule DROP CONSTRAINT schedule_workspace_id_flow_path_name_key;
DROP INDEX idx_schedule_flow;
ALTER TABLE schedule DROP COLUMN flow_path;
ALTER TABLE schedule ADD CONSTRAINT schedule_workspace_id_flow_id_name_key
    UNIQUE (workspace_id, flow_id, name);
CREATE INDEX idx_schedule_flow ON schedule (workspace_id, flow_id);
