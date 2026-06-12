-- Restore run_queue.last_ping (nullable, no default — matches the original
-- init schema). Historical values are not recoverable.
ALTER TABLE run_queue ADD COLUMN last_ping TIMESTAMPTZ;
