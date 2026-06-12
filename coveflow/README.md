# Backend

Rust workspace (Axum API + worker + queue)

## Prerequisites

- Rust 1.85+ (see `rust-toolchain.toml`)
- PostgreSQL 15+
- sqlx-cli: `cargo install sqlx-cli --no-default-features --features rustls,postgres`

## Environment

A `.env` file in this directory is auto-loaded at startup (via `dotenvy`).
Variables set in the shell take precedence over `.env`.

Minimal setup for local dev:

```bash
# coveflow/.env
DATABASE_URL=postgres://postgres:changeme@localhost/coveflow
JWT_SECRET=dev-secret-change-me
COVEFLOW_WORKER_NAME=worker-local
COVEFLOW_WORKER_TOTAL_CPUS=4
COVEFLOW_WORKER_TOTAL_MEMORY_MB=8192
COVEFLOW_WORKER_TOTAL_DISK_MB=102400
COVEFLOW_SANDBOX_MODE=none
```

## Backend Startup

The root binary starts the backend runtime. By default it runs both the HTTP API
and the worker in the same process.

```bash
# Start API and worker together
cargo run -p coveflow

# Start only the API server
cargo run -p coveflow -- --mode api

# Start only the worker
cargo run -p coveflow -- --mode worker
```

The runtime mode can also be configured with `COVEFLOW_MODE`:

```bash
COVEFLOW_MODE=api cargo run -p coveflow
COVEFLOW_MODE=worker cargo run -p coveflow
COVEFLOW_MODE=all cargo run -p coveflow
```

### Core configuration

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `DATABASE_URL` | yes (mode-dependent) | `postgres://postgres:changeme@localhost/coveflow` | PostgreSQL connection string |
| `JWT_SECRET` | **yes (api/all)** | — | JWT signing secret; missing value aborts startup |
| `COVEFLOW_API_ADDR` | no | `127.0.0.1:8000` | API bind address |
| `COVEFLOW_RUN_MIGRATIONS` | no | `true` | Run database migrations before starting |
| `COVEFLOW_DB_MAX_CONNECTIONS` | no | `10` (api/worker), `20` (all) | Database pool size |

### Worker configuration

All worker resource fields are **required** to avoid silently running with
unsafe defaults in production.

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `COVEFLOW_WORKER_NAME` | **yes (worker/all)** | — | Unique worker name (PK on `worker_ping`) |
| `COVEFLOW_WORKER_TOTAL_CPUS` | **yes (worker/all)** | — | Total CPU budget (e.g. `4`) |
| `COVEFLOW_WORKER_TOTAL_MEMORY_MB` | **yes (worker/all)** | — | Total memory budget |
| `COVEFLOW_WORKER_TOTAL_DISK_MB` | **yes (worker/all)** | — | Total disk budget |
| `COVEFLOW_SANDBOX_MODE` | **yes (worker/all)** | — | `none` / `dev` / `nsjail` / `k8s` — explicit choice prevents accidentally running untrusted code on the host |
| `COVEFLOW_WORKER_TAGS` | no | `default` | Comma-separated tag list |
| `COVEFLOW_WORKER_CLAIM_CONCURRENCY` | no | `4` | Number of parallel claim loops per worker |
| `COVEFLOW_WORKER_POLL_INTERVAL_SECS` | no | `1` | Claim poll interval (lower = lower job-pickup latency, higher DB load) |
| `COVEFLOW_WORKER_DEFAULT_RUN_TIMEOUT_SECS` | no | `3600` | Default per-run timeout |
| `COVEFLOW_WORKER_DIR` | no | OS temp dir | Working directory for run sandboxes |
| `COVEFLOW_WORKER_IP` | no | auto-detect | Override worker IP reported to `worker_ping` |

### Observability (all opt-in)

Nothing is shipped externally unless you set the corresponding env var.

| Variable | Default | Description |
|----------|---------|-------------|
| `COVEFLOW_METRICS_ADDR` | (disabled) | If set, exposes Prometheus metrics at `<addr>/metrics` (e.g. `0.0.0.0:9091`) |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | (disabled) | If set, sends OpenTelemetry traces via OTLP gRPC (e.g. `http://tempo:4317`) |
| `LOKI_ENDPOINT` | (disabled) | If set, ships logs to Loki (e.g. `http://loki:3100`) |
| `COVEFLOW_SERVICE_NAME` | derived from mode | Service name reported to OTel / Loki |
| `COVEFLOW_INSTANCE_ID` | hostname (fallback: `{service}-{pid}`) | Instance identifier used in DB log layer & OTel |
| `RUST_LOG` | `info,coveflow=info,coveflow_api=info,coveflow_worker=info,coveflow_queue=info` | Standard `tracing-subscriber` filter |

The DB log layer (writes to `service_log` for the frontend log viewer) is
always on — the frontend admin pages depend on it.

### Production checklist

- [ ] `JWT_SECRET` set to a strong random value
- [ ] `COVEFLOW_API_ADDR=0.0.0.0:8000` (or whatever your platform expects)
- [ ] `COVEFLOW_SANDBOX_MODE=nsjail` (`none` runs user code directly on the host)
- [ ] `COVEFLOW_WORKER_NAME` unique per worker instance
- [ ] `COVEFLOW_WORKER_TOTAL_*` sized to the host
- [ ] `COVEFLOW_METRICS_ADDR` set if Prometheus is scraping this instance
- [ ] `.env` files **not** committed (already in `.gitignore`)

## Commands

### Cargo

```bash
# Build all crates
cargo build

# Run API and worker together
cargo run -p coveflow

# Run with logging
RUST_LOG=info cargo run -p coveflow

# Debug a single subsystem
RUST_LOG=info,coveflow_worker=debug cargo run -p coveflow

# Run tests
cargo test

# Clippy lint
cargo clippy --workspace

# Format
cargo fmt --all
```

### SQLx

```bash
# Create database
sqlx database create

# Run migrations
sqlx migrate run

# Revert last migration
sqlx migrate revert

# Check queries at compile time (offline mode)
cargo sqlx prepare --workspace

# Reset database (drop + create + migrate)
sqlx database drop -y && sqlx database create && sqlx migrate run
```

### Migrations

```bash
# Create a new migration
sqlx migrate add <name>
```

Migration files are in `migrations/`. Each migration has `.up.sql` and `.down.sql`.

## Workspace Crates

| Crate | Description |
|-------|-------------|
| `crates/api` | Axum HTTP API server |
| `crates/queue` | Job queue (PostgreSQL-based) |
| `crates/worker` | Script execution worker |
| `crates/types` | Shared types (ScriptLang, RunKind, etc.) |
| `crates/flow-expr` | Flow expression language (parser + evaluator) |

## Architecture notes

- **No LISTEN/NOTIFY**: the codebase is fully PgBouncer transaction-pooling
  compatible. New runs, cancels, and completions are picked up via short
  polling intervals (default 200ms–1s depending on operation).
- **Concurrent claim loops**: each worker runs N parallel claim loops
  (`COVEFLOW_WORKER_CLAIM_CONCURRENCY`). Each loop pre-acquires a minimum
  resource slice before hitting the DB to prevent over-claiming.
- **Metrics**: API exports `http_requests_total` / `http_request_duration_seconds`;
  worker exports `queue_depth`, `active_workers`, `jobs_*_total`,
  `job_duration_seconds`, `job_queue_wait_seconds`, `resource_cpus_*`,
  `resource_memory_*`. All under a single registry exposed via the metrics
  server.
