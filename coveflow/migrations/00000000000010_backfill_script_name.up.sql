-- `name` is now a required field (enforced at the API). Backfill existing rows
-- that still have the empty default with the path's leaf segment, so older
-- scripts get a sensible non-empty display name.
UPDATE script SET name = regexp_replace(path, '^.*/', '') WHERE name = '';
