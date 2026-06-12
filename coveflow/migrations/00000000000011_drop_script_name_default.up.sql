-- The DEFAULT '' on script.name was only there to keep pre-existing rows valid
-- during the 0009 add + 0010 backfill. Now that every row has a real name and the
-- API requires a non-empty name on every insert, drop the default so a non-API
-- write can't silently create an empty-named (NOT NULL but '') row.
ALTER TABLE script ALTER COLUMN name DROP DEFAULT;
