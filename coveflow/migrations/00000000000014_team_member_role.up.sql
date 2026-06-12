-- Team membership gains a per-member role: `reader` (view files under the
-- team's `teams/<name>/` space) or `writer` (also edit). Existing members
-- default to `writer` so they keep full access.
ALTER TABLE team_member
    ADD COLUMN role VARCHAR(20) NOT NULL DEFAULT 'writer'
    CHECK (role IN ('reader', 'writer'));
