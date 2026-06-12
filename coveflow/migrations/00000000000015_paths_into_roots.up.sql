-- New permission model: every script/flow must live under one of three roots —
-- `users/<email>/`, `teams/<team>/`, or `workspace/`. Grandfather any legacy
-- free-form path (e.g. `f/etl/x`, `a/b`) into the shared `workspace/` root so it
-- stays visible + editable to all workspace members.
--
-- Safe: flows reference scripts by stable script_id (not path), so re-pathing
-- scripts doesn't break flow nodes; run history keeps its own path snapshot.
-- script.name is the path leaf, which the prefix doesn't change.
UPDATE script SET path = 'workspace/' || path
WHERE path NOT LIKE 'users/%' AND path NOT LIKE 'teams/%' AND path NOT LIKE 'workspace/%';

UPDATE flow SET path = 'workspace/' || path
WHERE path NOT LIKE 'users/%' AND path NOT LIKE 'teams/%' AND path NOT LIKE 'workspace/%';
