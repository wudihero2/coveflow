-- The cron occurrence ("logical date", Airflow-style) a scheduled run represents,
-- distinct from when it actually ran (created_at / started_at). NULL for manual
-- or ad-hoc runs. Lets the Runs view show which scheduled slot a run is for —
-- which matters when `catchup` backfills several occurrences in one tick (all
-- created "now" but each for a different slot).
ALTER TABLE run ADD COLUMN scheduled_time TIMESTAMPTZ;
