-- The end of the data interval (Airflow-style) a scheduled run covers: the next
-- cron occurrence after this run's `scheduled_time`. Together with
-- `scheduled_time` (= data_interval_start / logical_date) it gives the run's
-- window. NULL for manual/ad-hoc runs (their interval is zero-width at trigger
-- time, derived from created_at). Snapshotted at fire time so it stays correct
-- even if the schedule is later edited or deleted.
ALTER TABLE run ADD COLUMN data_interval_end TIMESTAMPTZ;
