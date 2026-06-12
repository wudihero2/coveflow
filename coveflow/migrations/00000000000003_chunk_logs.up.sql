-- Rebuild run_log as chunked structure for incremental SSE streaming.
-- Each chunk holds up to ~100 log entries as a JSONB array.

DROP TABLE IF EXISTS run_log;

CREATE TABLE run_log (
    id           BIGSERIAL PRIMARY KEY,
    run_id       UUID NOT NULL REFERENCES run(id),
    seq          INT NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    min_level    SMALLINT NOT NULL DEFAULT 3,
    max_level    SMALLINT NOT NULL DEFAULT 3,
    line_count   SMALLINT NOT NULL,
    entries      JSONB NOT NULL
);

CREATE TRIGGER trg_run_log_updated_at
    BEFORE UPDATE ON run_log FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- Primary query: fetch chunks for a specific run in order
CREATE INDEX idx_run_log_run_seq ON run_log (run_id, seq);

-- Fast path: filter chunks containing WARN/ERROR only
CREATE INDEX idx_run_log_level ON run_log (run_id, max_level) WHERE max_level >= 4;

-- Service-level logs (API/Worker/Scheduler process logs)
-- Same chunked strategy as run_log
CREATE TABLE service_log (
    id           BIGSERIAL PRIMARY KEY,
    instance_id  TEXT NOT NULL,
    service      TEXT NOT NULL,
    seq          INT NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    min_level    SMALLINT NOT NULL,
    max_level    SMALLINT NOT NULL,
    line_count   SMALLINT NOT NULL,
    entries      JSONB NOT NULL
);

CREATE TRIGGER trg_service_log_updated_at
    BEFORE UPDATE ON service_log FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE INDEX idx_service_log_instance ON service_log (instance_id, created_at);
CREATE INDEX idx_service_log_service  ON service_log (service, created_at);
