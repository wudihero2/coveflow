-- Drop bash support. Bash execution was never implemented in the worker, and
-- supporting shell scripts in the sandbox materially increases attack surface
-- (tool chain availability, fork bombs, easier secret exfiltration).
DELETE FROM script WHERE language = 'bash';

ALTER TABLE script DROP CONSTRAINT script_language_check;
ALTER TABLE script ADD CONSTRAINT script_language_check
    CHECK (language IN ('python3', 'typescript', 'duckdb'));
