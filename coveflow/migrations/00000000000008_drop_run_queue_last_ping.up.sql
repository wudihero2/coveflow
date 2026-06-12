-- Drop run_queue.last_ping: a dead-weight column. It was only ever set at claim
-- (now()) and cleared at unclaim (NULL) and never refreshed during execution or
-- read anywhere, so it duplicated started_at and could mislead future work into
-- thinking a per-job heartbeat existed. Worker liveness is tracked via
-- worker_ping.ping_at (see the liveness reaper, queue::reap_lost_workers).
ALTER TABLE run_queue DROP COLUMN last_ping;
