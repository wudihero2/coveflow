-- Restore original run_log schema and drop service_log

DROP TABLE IF EXISTS service_log;
DROP TABLE IF EXISTS run_log;

CREATE TABLE run_log (
    run_id UUID NOT NULL REFERENCES run(id),
    logs   TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (run_id)
);
