// =============================================================================
// CoveFlow Frontend Type Definitions
// =============================================================================
//
// This file mirrors the backend Rust structs so TypeScript knows the shape of
// every API request/response.  Keep it in sync with the backend types in:
//   coveflow/crates/api/src/  (auth.rs, scripts.rs, runs.rs, services.rs)
//   coveflow/crates/queue/src/log.rs
//   coveflow/crates/types/src/ (scripts.rs, run.rs)
//
// Naming convention:
//   - *Request  = body we POST to the backend
//   - *Response = body the backend returns
//   - *Item     = one row in a list response
//   - *Row      = raw DB row shape (logs)
// =============================================================================

// -----------------------------------------------------------------------------
// Auth  (backend: crates/api/src/auth.rs)
// -----------------------------------------------------------------------------

/** POST /api/auth/login and /api/auth/signup both return this. */
export interface AuthResponse {
	/** JWT access token — store in memory only, never in localStorage. */
	access_token: string;
	/** Token lifetime in seconds (backend default: 900 = 15 min). */
	expires_in: number;
	/** Email of the authenticated user. */
	email: string;
}

/** GET /api/workspaces/{id}/me — current user info within a workspace. */
export interface UserInfo {
	email: string;
	role: WorkspaceRole;
	/** Instance-wide admin (account.is_admin), distinct from the workspace role. */
	is_instance_admin: boolean;
	/** Teams the user belongs to (their `teams/<name>/` roots). */
	teams: string[];
	/** Subset of `teams` the user can write to (writer/owner role). */
	writable_teams: string[];
}

// -----------------------------------------------------------------------------
// Cluster dashboard  (backend: crates/api/src/cluster.rs)
// Cross-workspace, instance-admin only.
// -----------------------------------------------------------------------------

/** GET /api/admin/cluster/summary */
export interface ClusterSummary {
	total_cpus: number;
	used_cpus: number;
	/** used / total, in [0, 1]. */
	utilization: number;
	workers_total: number;
	workers_alive: number;
}

/** A used/total pair for one resource dimension. */
export interface ResourceUsage {
	used: number;
	total: number;
}

/** One worker row from GET /api/admin/cluster/workers */
export interface ClusterWorker {
	/** Unique per-process identity (used as the URL key). */
	worker: string;
	/** Operator-friendly name shown in the UI; may repeat across live processes. */
	display_name: string;
	tags: string[];
	sandbox_mode?: string;
	status: 'alive' | 'stale';
	ping_at: string;
	cpus: ResourceUsage;
	memory_mb: ResourceUsage;
	disk_mb: ResourceUsage;
	cpu_usage_percent: number;
	running_jobs: number;
}

/** One running job under a worker. */
export interface ClusterWorkerRun {
	run_id: string;
	workspace_id: string;
	script_path?: string;
	language?: string;
	cpus: number;
	memory_mb: number;
	disk_mb: number;
	started_at?: string;
	tag: string;
}

/** GET /api/admin/cluster/workers/{worker}/runs — bounded list. */
export interface ClusterWorkerRunsResponse {
	items: ClusterWorkerRun[];
	/** True when the result was truncated to the server's limit. */
	has_more: boolean;
}

// -----------------------------------------------------------------------------
// Script  (backend: crates/api/src/scripts.rs, crates/types/src/scripts.rs)
// -----------------------------------------------------------------------------

/**
 * Supported scripting languages.
 * Maps to Rust enum `ScriptLang` which serialises as lowercase strings.
 */
export type ScriptLang = 'python3';

/** POST /api/workspaces/{id}/scripts/create */
export interface CreateScriptRequest {
	/** Logical path like "utils/cleanup" — the identifier/namespace. */
	path: string;
	/** Human-readable display name (shown in lists + flow nodes). Required. */
	name: string;
	/** The actual source code. */
	content: string;
	/** Which language runtime to use for execution. */
	language: ScriptLang;
	/** Human-readable summary of what this version changed (like a commit msg). */
	summary?: string;
	/** Python pip packages needed, e.g. ["requests==2.31", "pandas"]. */
	requirements?: string[];
	/** Container image tag for execution, e.g. "python:3.12". Omit for platform default. */
	runtime?: string;
}

/**
 * Full script details — returned by get-by-hash and get-by-path.
 * Backend: ScriptResponse in scripts.rs
 */
export interface ScriptResponse {
	workspace_id: string;
	/**
	 * Content-addressable hash: SHA256(content + path + language + runtime + requirements).
	 * This is the primary key — every edit creates a new hash.
	 */
	hash: string;
	/** Stable per-script id (constant across versions and moves). Flows reference this. */
	script_id: string;
	path: string;
	/** Display name = the path's leaf. */
	name: string;
	content: string;
	language: string;
	/** Container image tag for execution, e.g. "python:3.12". null if using platform default. */
	runtime?: string;
	/**
	 * Auto-extracted schema from the script (e.g. function signatures).
	 * null if not yet analysed.
	 */
	schema: unknown | null;
	/**
	 * Hash(es) of the previous version(s) at the same path.
	 * Forms a linked list of version history. null for the first version.
	 */
	parent_hashes: string[] | null;
	summary: string;
	requirements: string[];
	/** Email of the user who created this version. */
	created_by: string;
	/** ISO 8601 timestamp, e.g. "2025-01-15T08:30:00Z". */
	created_at: string;
}

/**
 * Compact script info for the list view.
 * GET /api/workspaces/{id}/scripts/list returns an array of these.
 * Note: list_scripts uses DISTINCT ON(path) so only the latest version per path is returned.
 */
export interface ScriptListItem {
	hash: string;
	/** Stable per-script id (constant across versions and moves). */
	script_id: string;
	path: string;
	/** Display name = the path's leaf. */
	name: string;
	language: string;
	summary: string;
	created_by: string;
	created_at: string;
}

// -- Script version history --------------------------------------------------

/**
 * One entry in the version history list for a given script path.
 * GET /api/workspaces/{id}/scripts/history/{path}
 */
export interface ScriptVersionItem {
	hash: string;
	summary: string;
	created_by: string;
	created_at: string;
	parent_hashes: string[] | null;
}

/**
 * Paginated response for script version history.
 * Query params: ?limit=20&offset=0
 */
export interface ScriptVersionsResponse {
	items: ScriptVersionItem[];
	/** Total number of versions for this path (for pagination UI). */
	total: number;
	/** true if there are more versions beyond the current page. */
	has_more: boolean;
}

// -----------------------------------------------------------------------------
// Run  (backend: crates/api/src/runs.rs, crates/types/src/run.rs)
// -----------------------------------------------------------------------------

/**
 * What triggered the run.
 *   - script:       user clicked "Run" on a saved script
 *   - preview:      user ran code from the editor without saving
 *   - flow:         orchestrated multi-step workflow
 *   - flow_preview: preview of a flow
 *   - maintenance:  system maintenance task (e.g. cleanup)
 */
export type RunKind = 'script' | 'flow' | 'preview' | 'flow_preview' | 'maintenance';

/**
 * Run lifecycle states (derived in backend via `derive_status()`):
 *   queued    -> waiting for a worker to pick it up
 *   running   -> worker is executing the code
 *   success   -> completed with exit code 0
 *   failure   -> completed with non-zero exit code or error
 *   cancelled -> user or system cancelled before completion
 */
export type RunStatus = 'queued' | 'running' | 'success' | 'failure' | 'cancelled';

/** POST /api/workspaces/{id}/runs/create */
export interface CreateRunRequest {
	kind: RunKind;
	/** Hash of a saved script — mutually exclusive with raw_code. */
	script_hash?: string;
	/** Path of a saved script (backend resolves to latest version). */
	script_path?: string;
	/** Inline code for preview runs — mutually exclusive with script_hash. */
	raw_code?: string;
	language?: ScriptLang;
	/** Arguments passed to the script (arbitrary JSON). */
	args?: unknown;
	/** Routing tag for worker selection (e.g. "gpu", "high-mem"). */
	tag?: string;
	/** ISO 8601 timestamp for scheduled execution. */
	scheduled_for?: string;
	/** Higher = picked first. Default 0. */
	priority?: number;
	/** CPU limit (fractional cores, e.g. 0.5). */
	cpus?: number;
	/** Memory limit in MB. */
	memory_mb?: number;
	/** Disk limit in MB. */
	disk_mb?: number;
	/** Python pip packages to install before execution. */
	requirements?: string[];
	/** Max execution time in seconds. */
	timeout?: number;
	/** Docker image override. */
	custom_image?: string;
	/** Team to charge quotas against. */
	team_owner?: string;
}

/**
 * Full run details — returned by GET /api/workspaces/{id}/runs/{run_id}.
 * Backend: RunResponse in runs.rs
 */
export interface RunResponse {
	id: string;
	workspace_id: string;
	kind: RunKind;
	script_hash: string | null;
	script_path: string | null;
	raw_code: string | null;
	/** Flow execution progress (only present for kind=flow/flow_preview). */
	flow_status?: FlowRunState;
	/** Flow DAG definition (FlowSpec) for kind=flow/flow_preview, to render the graph. */
	flow_value?: FlowSpec;
	language: string | null;
	args: unknown | null;
	tag: string;
	/** Parent run ID (for sub-runs in flows). */
	parent_run: string | null;
	/** Root-level run ID (top of the flow tree). */
	root_run: string | null;
	requirements: string[];
	timeout: number | null;
	cpus: number;
	memory_mb: number;
	disk_mb: number;
	created_by: string;
	created_at: string;
	/** Derived status: queued | running | success | failure | cancelled */
	status: RunStatus;
	/** When a worker started executing. null if still queued. */
	started_at: string | null;
	/** When execution finished. null if still running/queued. */
	completed_at: string | null;
	/** Wall-clock time in milliseconds. null if not completed. */
	duration_ms: number | null;
	/** Script return value / output (arbitrary JSON). null if not completed. */
	result: unknown | null;
	/** Identifier of the user who cancelled this run. null if not cancelled. */
	canceled_by: string | null;
	/** Optional reason text provided when cancelling. null if no reason given. */
	canceled_reason: string | null;
	/** Identifier of the admin who marked this run's outcome. null if not marked. */
	marked_by: string | null;
	/** Optional reason text provided when marking. null if no reason given. */
	mark_reason: string | null;
	/** Airflow-style execution context (same `ctx` the script receives). */
	context: RunContext;
}

/**
 * Airflow-style execution context for a run.
 * Backend: RunContext in coveflow_types::run_context.
 * Every timestamp is a string; `ds`/`ts` are rendered in the schedule timezone.
 */
export interface RunContext {
	/** Start of the data interval (= logical_date), RFC3339 UTC. */
	data_interval_start: string;
	/** End of the data interval; equals logical_date for manual runs. */
	data_interval_end: string;
	/** The cron slot this run represents (= data_interval_start). */
	logical_date: string;
	/** logical_date as a date (YYYY-MM-DD) in the schedule timezone. */
	ds: string;
	/** logical_date as RFC3339 with offset in the schedule timezone. */
	ts: string;
	/** Schedule timezone (IANA); 'UTC' for manual runs. */
	timezone: string;
	run_id: string;
	/** Top-level flow run id; null for a standalone script run. */
	flow_run_id: string | null;
	/** Flow path; null for a standalone script run. */
	flow_path: string | null;
	created_by: string;
	is_scheduled: boolean;
	schedule_id: string | null;
	schedule_name: string | null;
	/** Wall-clock time the run was created (run.created_at), RFC3339 UTC. */
	triggered_at: string;
	/** The flow run's input/params; null for a standalone script run. */
	flow_input: unknown | null;
	/** Succeeded upstream node results: `{ <id>: { result } }`; {} for non-flow. */
	steps: Record<string, { result: unknown }>;
	/** Trigger provenance (webhook: `{ type, trigger_id, source_ip, ... }`); null
	 *  for manual / cron / flow-child runs. */
	trigger: Record<string, unknown> | null;
}

// -----------------------------------------------------------------------------
// Personal API Tokens (PAT)  (backend: crates/api/src/api_tokens.rs)
// -----------------------------------------------------------------------------

/** Token metadata — never includes the token value (GET /api/account/tokens). */
export interface ApiTokenListItem {
	id: string;
	name: string;
	created_at: string;
	last_used_at: string | null;
	expires_at: string | null;
}

/** Create response — the only place (besides reveal) the plaintext appears. */
export interface ApiTokenCreated {
	id: string;
	name: string;
	token: string;
	expires_at: string | null;
}

// -----------------------------------------------------------------------------
// Triggers  (backend: crates/api/src/triggers.rs)
// -----------------------------------------------------------------------------

/** One trigger (v1: webhook) for a flow. */
export interface Trigger {
	id: string;
	flow_id: string;
	trigger_type: string;
	name: string;
	enabled: boolean;
	config: Record<string, unknown>;
	created_by: string;
	created_at: string;
	updated_at: string;
	/** Path external callers POST to; prepend the app origin for the full URL. */
	webhook_path: string;
}

/** Create payload for a webhook trigger. */
export interface TriggerInput {
	name: string;
	trigger_type?: string;
	config?: Record<string, unknown>;
}

/** Partial update for a trigger. */
export interface TriggerUpdate {
	name?: string;
	enabled?: boolean;
	config?: Record<string, unknown>;
}

/**
 * Compact run info for the list view.
 * GET /api/workspaces/{id}/runs/list returns an array of these.
 */
export interface RunListItem {
	id: string;
	kind: RunKind;
	script_path: string | null;
	tag: string;
	status: RunStatus;
	created_by: string;
	created_at: string;
	started_at: string | null;
	completed_at: string | null;
	duration_ms: number | null;
	/** Explicit success flag — null if run hasn't completed yet. */
	success: boolean | null;
	/** For a flow's child run: the top-level flow run id, its flow path, and the
	 *  flow node that produced this run. All null for non-flow-child runs. */
	flow_run_id?: string | null;
	flow_path?: string | null;
	flow_step_id?: string | null;
	/** Airflow-style logical date: the cron slot a scheduled run is for; null otherwise. */
	scheduled_time?: string | null;
}

/** Response from POST /api/workspaces/{id}/scripts/create */
export interface ScriptCreatedResponse {
	hash: string;
}

/** Response from POST /api/workspaces/{id}/runs/create */
export interface RunCreatedResponse {
	id: string;
}

/** POST /api/workspaces/{id}/runs/{run_id}/cancel */
export interface CancelRunRequest {
	reason?: string;
	/** Force-kill a running container (default: graceful shutdown). */
	force?: boolean;
}

/** POST /api/workspaces/{id}/runs/{run_id}/rerun */
export interface RerunRequest {
	/** If true, resolve script_path to the latest version instead of reusing the original hash. */
	use_latest_version?: boolean;
}

/** POST /api/workspaces/{id}/runs/{run_id}/mark-success or mark-fail */
export interface MarkRunRequest {
	/** Why the admin is manually marking this run. */
	reason?: string;
	/** Override result value. */
	result?: unknown;
}

// -----------------------------------------------------------------------------
// Logs  (backend: crates/queue/src/log.rs, crates/api/src/runs.rs)
// -----------------------------------------------------------------------------

/**
 * A single log line within a chunk.
 * Chunks batch multiple log lines together for efficiency.
 *
 * Produced by the worker's DbLogLayer (crates/worker/src/db_log.rs).
 * Each tracing event is serialised as: { ts, level, msg, target, fields? }
 */
export interface LogEntry {
	/** ISO 8601 timestamp (RFC 3339). */
	ts: string;
	/**
	 * Numeric severity level:
	 *   1=trace, 2=debug, 3=info, 4=warn, 5=error
	 */
	level: number;
	/** The log message text. */
	msg: string;
	/** Tracing target (module path), e.g. "coveflow_worker::python". */
	target?: string;
	/** Additional structured fields attached to the tracing event. */
	fields?: Record<string, unknown>;
	/** For flow runs: which node in the flow produced this line (future use). */
	node_id?: string;
}

/**
 * Log chunk as sent by the SSE stream (event: "log").
 * This is a slim shape — the SSE endpoint (runs.rs:486) only sends
 * { chunk_id, seq, entries }, NOT the full RunLogChunkRow.
 *
 * Used by the SSE helper's `onLog` callback.
 */
export interface LogChunk {
	chunk_id: number;
	seq: number;
	entries: LogEntry[];
}

/**
 * SSE "result" event data — sent when the run completes.
 * After this event, the SSE stream closes.
 */
export interface RunResultEvent {
	success: boolean;
	result: unknown | null;
}

/**
 * Raw DB row for run log chunks.
 * Backend: RunLogChunkRow in queue/src/log.rs
 *
 * Logs are stored in "chunks" (batches of lines) rather than
 * individual lines, to reduce DB row count and improve streaming perf.
 */
export interface RunLogChunkRow {
	/** Auto-increment chunk ID (used as cursor for pagination). */
	id: number;
	run_id: string;
	/** Sequence number within this run (0, 1, 2, ...). */
	seq: number;
	created_at: string;
	/** Minimum log level in this chunk (for filtering). */
	min_level: number;
	/** Maximum log level in this chunk (for filtering). */
	max_level: number;
	/** Number of LogEntry items in this chunk. */
	line_count: number;
	/** JSON array of LogEntry objects. */
	entries: LogEntry[];
}

/**
 * Present in RunLogsResponse when the run is in `run_completed`.
 * Polling clients use this to know when to stop and what payload to surface.
 */
export interface RunCompletedInfo {
	success: boolean;
	result: unknown | null;
}

/**
 * GET /api/workspaces/{id}/runs/{run_id}/logs
 * Returns historical log chunks with cursor-based pagination,
 * plus the current run status so a polling client can self-terminate.
 */
export interface RunLogsResponse {
	run_id: string;
	chunks: RunLogChunkRow[];
	/**
	 * Pass this as ?after_chunk= in the next request to get more chunks.
	 * null means no more data.
	 */
	next_cursor: number | null;
	/** "queued" | "running" | "success" | "failure" | "cancelled" */
	status: string;
	/** Present iff the run has finished. */
	completed: RunCompletedInfo | null;
}

// -----------------------------------------------------------------------------
// Service Logs  (backend: crates/api/src/services.rs)
// -----------------------------------------------------------------------------

/**
 * Raw DB row for service log chunks (worker/api/scheduler process logs).
 * Backend: ServiceLogChunkRow in queue/src/log.rs
 */
export interface ServiceLogChunkRow {
	id: number;
	/** Unique identifier for the running process instance. */
	instance_id: string;
	/** Service name, e.g. "worker", "api", "scheduler". */
	service: string;
	seq: number;
	created_at: string;
	min_level: number;
	max_level: number;
	line_count: number;
	/** JSON array of LogEntry objects. */
	entries: LogEntry[];
}

/**
 * GET /api/workspaces/{id}/services/logs
 * Query params: ?service=&instance=&level=&after_chunk=&limit=
 */
export interface ServiceLogsResponse {
	chunks: ServiceLogChunkRow[];
	next_cursor: number | null;
}

/**
 * SSE "log" event data for service log streams.
 * Sent by GET /api/workspaces/{id}/services/logs/stream
 */
export interface ServiceLogChunk {
	chunk_id: number;
	seq: number;
	instance_id: string;
	service: string;
	entries: LogEntry[];
}

// -----------------------------------------------------------------------------
// Admin: Workspace  (backend: DB schema workspace + workspace_member tables)
// -----------------------------------------------------------------------------

/**
 * Workspace member roles — controls what a user can do:
 *   admin    = full control (manage members, teams, folders, settings)
 *   editor   = create/edit scripts, create/cancel runs
 *   viewer   = read-only access to scripts, runs, logs
 *   operator = run/rerun/cancel jobs, mark-success/fail, but no script editing
 */
export type WorkspaceRole = 'admin' | 'editor' | 'viewer' | 'operator';

export interface WorkspaceMember {
	email: string;
	role: WorkspaceRole;
}

export interface WorkspaceInfo {
	id: string;
	name: string;
	owner: string;
}

// -----------------------------------------------------------------------------
// Admin: Team  (backend: DB schema team + team_member + team_quota tables)
// -----------------------------------------------------------------------------

export interface TeamItem {
	name: string;
	summary: string;
}

/**
 * GET /api/workspaces/{id}/teams/list
 * Returns the teams the authenticated user belongs to in this workspace.
 */
export interface TeamListItem {
	name: string;
	summary: string;
	member_count: number;
}

export interface TeamListResponse {
	items: TeamListItem[];
}

/** A team member with their access role for the team's `teams/<name>/` space. */
export interface TeamMember {
	email: string;
	role: 'reader' | 'writer';
}

export interface UserSearchItem {
	email: string;
}

/**
 * Resource quotas enforced per team.
 * null means "no limit" (inherits workspace defaults).
 * Enforcement happens in the job queue's claim logic (queue/src/claim.rs).
 */
export interface TeamQuota {
	max_concurrent_runs: number | null;
	max_cpus: number | null;
	max_memory_mb: number | null;
	max_daily_runs: number | null;
	max_storage_bytes: number | null;
	max_run_timeout_secs: number | null;
}

// ---------------------------------------------------------------------------
// Flows (DAG). Mirrors coveflow-types::flows / flow_status.
// ---------------------------------------------------------------------------

export type InputBinding =
	| { kind: 'static'; value: unknown }
	| { kind: 'expr'; expr: string };

export type NodeBody =
	| { kind: 'script'; script_id: string; hash?: string; inputs?: Record<string, InputBinding>; queue?: string }
	| { kind: 'branch'; task: NodeBody };

/** A Branch node's outgoing-edge routing case (see FlowEdge.case). */
export type BranchCase =
	| { kind: 'match'; value: unknown }
	| { kind: 'default' };

export type Backoff =
	| { kind: 'fixed'; delay_ms: number }
	| { kind: 'exponential'; base_ms: number; factor: number; jitter?: number };

export interface RetryPolicy {
	max_attempts: number;
	backoff: Backoff;
}

/** Editor canvas position; ignored by the engine. */
export interface NodePos {
	x: number;
	y: number;
}

/** Fan-in trigger rule: how a node aggregates its upstreams' terminal states.
 *  Absent = 'all_success'. Mirrors coveflow_types::flows::TriggerRule. */
export type TriggerRule =
	| 'all_success'
	| 'none_failed_min_one_success'
	| 'all_done'
	| 'all_failed';

export interface FlowNode {
	id: string;
	body: NodeBody;
	retry?: RetryPolicy;
	summary?: string;
	skip_if?: string;
	trigger_rule?: TriggerRule;
	ui?: NodePos;
}

export interface FlowEdge {
	from: string;
	to: string;
	/** Conditional edge: only active when this expression is truthy. */
	when?: string;
	/** Branch routing case (only on edges whose source is a Branch node). */
	case?: BranchCase;
	/** Editor-only: source/target node handle (t/l/b/r) the edge attaches to,
	 *  so routing survives save/run/reload. Ignored by the engine. */
	from_handle?: string;
	to_handle?: string;
}

export interface FlowSpec {
	nodes: FlowNode[];
	edges: FlowEdge[];
	on_error?: FlowNode;
	max_concurrent?: number;
	/** Flow-level default retry; a node's own `retry` overrides it (a node sets
	 *  max_attempts: 0 to opt out). */
	retry?: RetryPolicy;
}

export interface FlowListItem {
	flow_id: string;
	path: string;
	revision: number;
	summary: string;
	edited_by: string;
	edited_at: string;
}

// -----------------------------------------------------------------------------
// Schedules (cron)  (backend: crates/api/src/schedules.rs)
// -----------------------------------------------------------------------------

/** Most recent run of a schedule (for the list's status dot). */
export interface ScheduleLastRun {
	id: string;
	created_at: string;
	/** true/false once terminal; null while queued/running. */
	success: boolean | null;
}

/** One row in GET /schedules/list. */
export interface ScheduleListItem {
	id: string;
	name: string;
	flow_id: string;
	/** Current path of the flow (resolved from flow_id), for display. */
	flow_path: string;
	cron_expr: string;
	timezone: string;
	enabled: boolean;
	catchup: boolean;
	max_active_runs: number | null;
	next_trigger_at: string | null;
	last_triggered_at: string | null;
	last_error: string | null;
	last_run: ScheduleLastRun | null;
}

/** Full schedule (GET /schedules/get/{id}). */
export interface Schedule {
	id: string;
	workspace_id: string;
	name: string;
	flow_id: string;
	/** Current path of the flow (resolved from flow_id), for display. */
	flow_path: string;
	cron_expr: string;
	timezone: string;
	args: unknown;
	enabled: boolean;
	catchup: boolean;
	max_active_runs: number | null;
	next_trigger_at: string | null;
	last_triggered_at: string | null;
	last_error: string | null;
	created_by: string;
	created_at: string;
	updated_at: string;
}

/** Body for create/update schedule. */
export interface ScheduleInput {
	name: string;
	flow_id: string;
	cron_expr: string;
	timezone?: string;
	args?: unknown;
	enabled?: boolean;
	catchup?: boolean;
	max_active_runs?: number | null;
}


export interface FlowResponse {
	flow_id: string;
	path: string;
	revision: number;
	summary: string;
	value: FlowSpec;
	edited_by: string;
	edited_at: string;
}

export interface CreateFlowRequest {
	path: string;
	value: FlowSpec;
	summary?: string;
}

// -- Flow runtime state (run.flow_status) -----------------------------------

export type NodeRunState =
	| { state: 'pending'; id: string }
	| { state: 'running'; id: string; run_id?: string; fanout?: Fanout }
	| { state: 'succeeded'; id: string; run_id?: string; result: unknown }
	| { state: 'failed'; id: string; run_id?: string; error: unknown }
	| { state: 'skipped'; id: string };

export interface Fanout {
	total: number;
	completed: number;
	children: string[];
	failed?: string[];
	items?: unknown[];
}

export interface FlowRunState {
	nodes: NodeRunState[];
	on_error?: NodeRunState;
	retries?: Record<string, number>;
}

/**
 * Secret metadata. The store is write-only — the value is never
 * returned by the API; only the worker decrypts it for injection.
 * Backend: SecretListItem in crates/api/src/secrets.rs.
 */
export interface SecretListItem {
	/** Three-root path, e.g. `workspace/openai_api_key`. */
	path: string;
	description: string;
	created_by: string;
	updated_by: string;
	updated_at: string;
}

/** Create payload for a new secret. */
export interface SecretInput {
	path: string;
	value: string;
	description?: string;
}

/** Rotate payload (path is in the URL; value is replaced). */
export interface SecretRotateInput {
	value: string;
	description?: string;
}
