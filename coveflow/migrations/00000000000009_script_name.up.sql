-- Give scripts a human-readable display name, distinct from the logical `path`.
-- The path stays the identifier/namespace; `name` is what UIs (script list, flow
-- nodes) show. Stored per version (like `summary`); the latest version's name is
-- the display name. Default '' keeps existing rows valid; readers fall back to
-- the path when empty.
ALTER TABLE script ADD COLUMN name VARCHAR(255) NOT NULL DEFAULT '';
