-- Best-effort reverse: strip the `workspace/` prefix. Cannot distinguish
-- grandfathered paths from genuinely workspace-created ones, so this strips all
-- of them (acceptable for the dev-only rollback this targets).
UPDATE script SET path = substring(path FROM 11) WHERE path LIKE 'workspace/%';
UPDATE flow SET path = substring(path FROM 11) WHERE path LIKE 'workspace/%';
