-- Partial index supporting the cluster dashboard's per-worker queries:
--   - cluster_workers: COUNT(*) ... GROUP BY worker WHERE running = TRUE
--   - cluster_worker_runs: WHERE worker = $1 AND running = TRUE
-- The existing idx_run_queue_pull is WHERE running = FALSE (opposite polarity),
-- so without this both queries seq-scan run_queue.
CREATE INDEX idx_run_queue_worker ON run_queue (worker) WHERE running = TRUE;
