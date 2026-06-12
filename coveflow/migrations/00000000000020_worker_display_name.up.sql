-- Split worker identity from its display name. `worker` (PK) is now a unique
-- per-process identity (configured name + a random suffix), so a restarted
-- worker is a NEW row: the old process's row goes stale and the reaper reclaims
-- its orphaned runs instead of being shadowed by a same-named fresh heartbeat.
-- `display_name` is the operator-friendly name shown in the UI (several live
-- processes can share one display_name).
ALTER TABLE worker_ping ADD COLUMN display_name VARCHAR(100);

-- Backfill existing rows: their identity was already the friendly name.
UPDATE worker_ping SET display_name = worker WHERE display_name IS NULL;
