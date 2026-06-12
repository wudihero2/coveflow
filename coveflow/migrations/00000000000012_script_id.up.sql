-- Stable per-logical-script identity, independent of path (which becomes a
-- movable label). All versions of a script that share (workspace_id, path) get
-- the same script_id; flows will reference script_id instead of path so that
-- moving/renaming a script never breaks references.
ALTER TABLE script ADD COLUMN script_id UUID;

-- Assign one id per logical script (one per (workspace_id, path) group), applied
-- to every version row of that path.
UPDATE script s
SET script_id = sub.id
FROM (
    SELECT workspace_id, path, gen_random_uuid() AS id
    FROM script
    GROUP BY workspace_id, path
) sub
WHERE s.workspace_id = sub.workspace_id AND s.path = sub.path;

ALTER TABLE script ALTER COLUMN script_id SET NOT NULL;

-- Resolve "latest version of this logical script" by (workspace_id, script_id).
CREATE INDEX script_workspace_script_id_idx ON script (workspace_id, script_id);
