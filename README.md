<div align="center">

# CoveFlow

**Self-hostable platform to run Python scripts, compose them into DAG flows, and schedule them on a sandboxed, resource-aware worker pool.**

[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
![Rust](https://img.shields.io/badge/backend-Rust-orange.svg)
![SvelteKit](https://img.shields.io/badge/frontend-SvelteKit-ff3e00.svg)
![Status](https://img.shields.io/badge/status-early%20development-yellow.svg)

</div>

> [!WARNING]
> **Status: early development (v0.x).** Schema and APIs may change, and it is not
> yet hardened for production. Try it out, file issues, and PRs are welcome.

---

## What is CoveFlow?

CoveFlow lets you write small Python scripts, wire them together into **flows**
(directed acyclic graphs) in a visual editor, and run them on demand or on a
**cron schedule** — all self-hosted. Scripts execute on a pool of workers that
track CPU/memory/disk and isolate each job in a sandbox. Everything is
multi-tenant (workspaces, teams) with a path-based permission model and
first-class observability.

## Features

- **Scripts** — author versioned Python scripts; `main()` parameters are auto-detected into typed inputs.
- **Flows** — drag-and-drop DAG editor: script & branch nodes, per-node retries, Airflow-style trigger rules, and on-error handlers. No JSON authoring required.
- **Scheduling** — timezone-aware cron (DST-correct), optional catch-up for missed ticks, and sub-minute intervals (down to 10s).
- **Worker pool** — jobs declare `cpus` / `memory_mb` / `disk_mb`; workers track three-dimensional capacity and execute inside an **nsjail** sandbox.
- **Permissions** — three roots: personal `users/<you>/`, team `teams/<team>/`, and shared `workspace/`, with read/write ACLs reused across scripts, flows, and schedules.
- **Runs & logs** — full run history, live streaming logs, per-run metadata, and lineage showing which flow a child run came from. Display timezone is a per-user UI preference (storage stays UTC).
- **Multi-tenant** — workspaces, teams, roles, and per-team quotas.
- **Observability** — Prometheus metrics, OpenTelemetry traces (Tempo), and Loki logs, with ready-made Grafana dashboards under [`infra/`](infra/).

## Architecture

A single Rust binary (`coveflow-server`) runs as **API**, **worker**, or **both**
(`--mode api|worker|all`); the SvelteKit frontend talks to it over HTTP.

```
            ┌──────────────┐        ┌───────────────────────────┐
            │  SvelteKit   │  HTTP  │      coveflow-server       │
 browser ──▶│  frontend    │ ─────▶ │  ┌────────┐  ┌──────────┐  │
            │ (vite :5173) │  /api  │  │  API   │  │ scheduler│  │
            └──────────────┘        │  └────────┘  └──────────┘  │
                                    │  ┌────────┐  ┌──────────┐  │
                                    │  │ worker │  │  reaper  │  │
                                    │  └───┬────┘  └──────────┘  │
                                    └──────┼────────────────────┘
                                           │ nsjail sandbox
                                   ┌───────▼────────┐   ┌──────────────┐
                                   │   Python job   │   │  PostgreSQL  │
                                   └────────────────┘   └──────────────┘
```

**Rust workspace** (`coveflow/`):

| Crate | Responsibility |
|---|---|
| `crates/api` | HTTP API, auth, workspaces/teams, scripts, flows, schedules, runs |
| `crates/queue` | Run queue, flow DAG engine, cron scheduler |
| `crates/worker` | Job execution, resource manager, nsjail sandbox, Python runtime |
| `crates/types` | Shared domain types (serde) |
| `crates/flow-expr` | Restricted expression evaluator for flow inputs |

## Quick start (local dev)

**Prerequisites:** Docker (for Postgres + observability), Node 20+, and Rust
(the pinned toolchain in `coveflow/rust-toolchain.toml` is installed
automatically). The **nsjail** sandbox is Linux-only — on macOS use
`COVEFLOW_SANDBOX_MODE=none` for local development (or run the server inside a
Linux VM).

```bash
git clone git@github.com:wudihero2/coveflow.git
cd coveflow

# 1. Start dependencies (Postgres; plus the observability stack)
docker compose up -d postgres        # or `docker compose up -d` for the full stack

# 2. Run the server (API + worker + scheduler in one process).
#    Migrations run automatically on startup.
cd coveflow
DATABASE_URL=postgres://postgres:changeme@localhost/coveflow \
JWT_SECRET=dev-secret-change-me \
COVEFLOW_SANDBOX_MODE=none \
COVEFLOW_WORKER_NAME=dev-worker \
COVEFLOW_INSTANCE_ADMIN_EMAIL=admin@example.com \
COVEFLOW_INSTANCE_ADMIN_PASSWORD=changeme123 \
cargo run --bin coveflow-server -- --mode all
# API listens on 127.0.0.1:8000

# 3. In another terminal, run the frontend
cd frontend
npm install
npm run dev          # http://localhost:5173 (proxies /api → :8000)
```

Then open <http://localhost:5173> and log in with the bootstrapped admin above.

## Configuration

Set via environment variables (the server fails fast on missing required ones):

| Variable | Required | Description |
|---|---|---|
| `DATABASE_URL` | yes | PostgreSQL connection string |
| `JWT_SECRET` | api/all | Signing secret for auth tokens |
| `COVEFLOW_SANDBOX_MODE` | worker/all | `none` (dev) or `nsjail` (production, Linux) |
| `COVEFLOW_WORKER_NAME` | worker/all | Unique worker identity |
| `COVEFLOW_MODE` | no | `api` / `worker` / `all` (default `all`; CLI `--mode` wins) |
| `COVEFLOW_API_ADDR` | no | API bind address (default `127.0.0.1:8000`) |
| `COVEFLOW_METRICS_ADDR` | no | Enables the Prometheus `/metrics` server when set |
| `COVEFLOW_INSTANCE_ADMIN_EMAIL` / `_PASSWORD` | no | Bootstrap an admin on first start |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | no | Export traces (e.g. to Tempo) |
| `LOKI_ENDPOINT` | no | Ship logs to Loki |
| `COVEFLOW_RUN_MIGRATIONS` | no | Run DB migrations on startup (default `true`) |

## Project layout

```
coveflow/        Rust workspace (crates/* + src/main.rs → coveflow-server)
frontend/        SvelteKit app (Svelte 5 runes, Tailwind v4)
infra/           Prometheus, Tempo, and Grafana dashboards
docker-compose.yml   Postgres + observability stack
```

## Development

```bash
# Backend (from coveflow/)
cargo test --workspace
cargo clippy --workspace --lib --bins -- -D warnings
cargo fmt --all

# Frontend (from frontend/)
npm run check        # svelte-check (0 errors expected)
```

## Contributing

Issues and pull requests are welcome. By contributing, you agree your
contributions are licensed under the project's Apache-2.0 license (Apache-2.0
§5). Please keep `cargo clippy`, `cargo fmt`, and `npm run check` clean.

## License

Licensed under the [Apache License, Version 2.0](LICENSE).
Copyright 2026 The CoveFlow Authors.
