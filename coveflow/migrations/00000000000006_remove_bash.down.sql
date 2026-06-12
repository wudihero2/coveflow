ALTER TABLE script DROP CONSTRAINT script_language_check;
ALTER TABLE script ADD CONSTRAINT script_language_check
    CHECK (language IN ('python3', 'typescript', 'bash', 'duckdb'));
