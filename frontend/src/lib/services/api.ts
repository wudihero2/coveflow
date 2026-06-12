import type {
	CancelRunRequest,
	ClusterSummary,
	ClusterWorker,
	ClusterWorkerRunsResponse,
	CreateFlowRequest,
	CreateRunRequest,
	CreateScriptRequest,
	FlowListItem,
	FlowResponse,
	MarkRunRequest,
	Schedule,
	ScheduleInput,
	ScheduleListItem,
	RerunRequest,
	RunCreatedResponse,
	RunListItem,
	RunLogsResponse,
	RunResponse,
	ScriptCreatedResponse,
	ScriptListItem,
	ScriptResponse,
	ScriptVersionsResponse,
	SecretInput,
	SecretListItem,
	SecretRotateInput,
	ApiTokenListItem,
	ApiTokenCreated,
	Trigger,
	TriggerInput,
	TriggerUpdate,
	ServiceLogsResponse,
	TeamListItem,
	TeamMember,
	TeamListResponse,
	TeamQuota,
	UserInfo,
	UserSearchItem,
	WorkspaceInfo,
	WorkspaceMember,
	WorkspaceRole
} from '$lib/types';
import { auth, tryRefresh } from '$lib/stores/auth.svelte';
import { buildQuery, encodePath } from '$lib/services/url';

// Extra request option for endpoints that intentionally do not use JWT auth.
type ApiRequestInit = RequestInit & {
	auth?: boolean;
};

// Typed HTTP error so UI code can show status/body instead of a generic Error.
export class ApiClientError extends Error {
	constructor(
		message: string,
		public readonly status: number,
		public readonly body: string
	) {
		super(message);
		this.name = 'ApiClientError';
	}
}

// Shared fetch wrapper:
// - adds Authorization from the auth store
// - defaults JSON bodies to application/json
// - retries once after a successful refresh when access token expired
async function request<T>(path: string, init: ApiRequestInit = {}, retry = true): Promise<T> {
	const headers = new Headers(init.headers);
	const includeAuth = init.auth !== false;

	if (includeAuth && auth.token) {
		headers.set('Authorization', `Bearer ${auth.token}`);
	}

	if (init.body && !headers.has('Content-Type')) {
		headers.set('Content-Type', 'application/json');
	}

	const response = await fetch(path, {
		...init,
		headers
	});

	if (response.status === 401 && retry && includeAuth && (await tryRefresh())) {
		return request<T>(path, init, false);
	}

	if (!response.ok) {
		const body = await response.text();
		throw new ApiClientError(body || response.statusText, response.status, body);
	}

	if (response.status === 204) {
		return undefined as T;
	}

	return response.json() as Promise<T>;
}

// Log level names accepted by the backend's parse_level().
export type LogLevelName = 'TRACE' | 'DEBUG' | 'INFO' | 'WARN' | 'ERROR';

// -----------------------------------------------------------------------------
// WorkspaceApi — all workspace-scoped methods pinned to an explicit ID.
// -----------------------------------------------------------------------------

export interface WorkspaceApi {
	readonly workspaceId: string;

	// Scripts
	listScripts(pathPrefix?: string): Promise<ScriptListItem[]>;
	createScript(req: CreateScriptRequest): Promise<ScriptCreatedResponse>;
	getScriptByHash(hash: string): Promise<ScriptResponse>;
	getScriptByPath(path: string): Promise<ScriptResponse>;
	moveScript(
		scriptId: string,
		newPath: string,
		overwrite?: boolean
	): Promise<{ path?: string; error?: string; referenced_by?: string[] }>;
	deleteScript(
		scriptId: string,
		force?: boolean
	): Promise<{ deleted?: boolean; error?: string; referenced_by?: string[] }>;
	scriptReferences(scriptId: string): Promise<{ flows: string[]; active_runs: boolean }>;
	listScriptVersions(
		path: string,
		limit?: number,
		offset?: number
	): Promise<ScriptVersionsResponse>;

	// Flows
	listFlows(): Promise<FlowListItem[]>;
	getFlow(path: string): Promise<FlowResponse>;
	createFlow(req: CreateFlowRequest): Promise<{ path: string; revision: number }>;
	moveFlow(
		oldPath: string,
		newPath: string,
		overwrite?: boolean
	): Promise<{ path?: string; error?: string }>;
	deleteFlow(path: string, force?: boolean): Promise<{ deleted?: boolean; error?: string }>;
	runFlow(path: string, args?: unknown): Promise<RunCreatedResponse>;
	checkExpr(expr: string): Promise<{ ok: boolean; error?: string }>;

	// Schedules (cron)
	listSchedules(): Promise<ScheduleListItem[]>;
	getSchedule(id: string): Promise<Schedule>;
	createSchedule(req: ScheduleInput): Promise<{ id: string }>;
	updateSchedule(id: string, req: ScheduleInput): Promise<{ id: string }>;
	deleteSchedule(id: string): Promise<{ deleted?: boolean }>;
	setScheduleEnabled(id: string, enabled: boolean): Promise<{ enabled: boolean }>;
	runScheduleNow(id: string): Promise<RunCreatedResponse>;
	previewSchedule(
		cronExpr: string,
		timezone?: string,
		count?: number
	): Promise<{ next: string[] }>;

	// Secrets (write-only encrypted store)
	listSecrets(): Promise<SecretListItem[]>;
	createSecret(req: SecretInput): Promise<{ path: string }>;
	rotateSecret(path: string, req: SecretRotateInput): Promise<void>;
	deleteSecret(path: string): Promise<void>;

	// Triggers (webhook etc.) — per flow
	listTriggers(flowId: string): Promise<Trigger[]>;
	createTrigger(flowId: string, req: TriggerInput): Promise<Trigger>;
	updateTrigger(id: string, req: TriggerUpdate): Promise<void>;
	deleteTrigger(id: string): Promise<void>;

	// Runs
	createRun(req: CreateRunRequest): Promise<RunCreatedResponse>;
	runWaitResult(req: CreateRunRequest, timeout?: number): Promise<RunResponse>;
	listRuns(query?: {
		status?: string;
		kind?: string;
		script_path?: string;
		created_by?: string;
		created_after_ms?: number;
		created_before_ms?: number;
		schedule_id?: string;
			trigger_id?: string;
		sort?: string;
		order?: 'asc' | 'desc';
		limit?: number;
		offset?: number;
	}): Promise<RunListItem[]>;
	getRun(runId: string): Promise<RunResponse>;
	getRunLogs(
		runId: string,
		query?: { level?: LogLevelName; after_chunk?: number; limit?: number }
	): Promise<RunLogsResponse>;
	cancelRun(runId: string, req?: CancelRunRequest): Promise<{ outcome: string }>;
	rerun(runId: string, req?: RerunRequest): Promise<RunCreatedResponse>;
	markSuccess(runId: string, req?: MarkRunRequest): Promise<void>;
	markFail(runId: string, req?: MarkRunRequest): Promise<void>;

	// Service Logs
	getServiceLogs(query?: {
		service?: string;
		instance?: string;
		level?: LogLevelName;
		after_chunk?: number;
		limit?: number;
		since_ms?: number;
	}): Promise<ServiceLogsResponse>;

	// Teams
	listTeams(): Promise<TeamListResponse>;
	getTeam(name: string): Promise<TeamListItem>;
	createTeam(name: string, summary?: string): Promise<void>;
	deleteTeam(name: string): Promise<void>;
	listTeamMembers(name: string): Promise<TeamMember[]>;
	addTeamMember(name: string, email: string, role: 'reader' | 'writer'): Promise<void>;
	updateTeamMemberRole(name: string, email: string, role: 'reader' | 'writer'): Promise<void>;
	removeTeamMember(name: string, email: string): Promise<void>;
	getTeamQuota(name: string): Promise<TeamQuota>;
	updateTeamQuota(name: string, quota: TeamQuota): Promise<void>;

	// Members
	listMembers(): Promise<WorkspaceMember[]>;
	addMember(email: string, role: WorkspaceRole): Promise<void>;
	updateMemberRole(email: string, role: WorkspaceRole): Promise<void>;
	removeMember(email: string): Promise<void>;

	// User
	getMe(): Promise<UserInfo>;
}

function createWorkspaceApi(workspaceId: string): WorkspaceApi {
	const base = `/api/workspaces/${workspaceId}`;

	return {
		workspaceId,

		// -- Scripts -----------------------------------------------------------

		listScripts(pathPrefix?: string): Promise<ScriptListItem[]> {
			return request(`${base}/scripts/list${buildQuery({ path_prefix: pathPrefix })}`);
		},

		createScript(req: CreateScriptRequest): Promise<ScriptCreatedResponse> {
			return request(`${base}/scripts/create`, {
				method: 'POST',
				body: JSON.stringify(req)
			});
		},

		getScriptByHash(hash: string): Promise<ScriptResponse> {
			return request(`${base}/scripts/get/hash/${hash}`);
		},

		getScriptByPath(path: string): Promise<ScriptResponse> {
			return request(`${base}/scripts/get/path/${encodePath(path)}`);
		},

		moveScript(
			scriptId: string,
			newPath: string,
			overwrite?: boolean
		): Promise<{ path?: string; error?: string; referenced_by?: string[] }> {
			return request(`${base}/scripts/move`, {
				method: 'POST',
				body: JSON.stringify({ script_id: scriptId, new_path: newPath, overwrite })
			});
		},

		deleteScript(
			scriptId: string,
			force?: boolean
		): Promise<{ deleted?: boolean; error?: string; referenced_by?: string[] }> {
			return request(`${base}/scripts/delete`, {
				method: 'POST',
				body: JSON.stringify({ script_id: scriptId, force })
			});
		},

		scriptReferences(scriptId: string): Promise<{ flows: string[]; active_runs: boolean }> {
			return request(`${base}/scripts/references/${scriptId}`);
		},

		listScriptVersions(
			path: string,
			limit?: number,
			offset?: number
		): Promise<ScriptVersionsResponse> {
			return request(
				`${base}/scripts/history/${encodePath(path)}${buildQuery({ limit, offset })}`
			);
		},

		// -- Flows ------------------------------------------------------------

		listFlows(): Promise<FlowListItem[]> {
			return request(`${base}/flows/list`);
		},

		getFlow(path: string): Promise<FlowResponse> {
			return request(`${base}/flows/get/${encodePath(path)}`);
		},

		createFlow(req: CreateFlowRequest): Promise<{ path: string; revision: number }> {
			return request(`${base}/flows/create`, {
				method: 'POST',
				body: JSON.stringify(req)
			});
		},

		moveFlow(
			oldPath: string,
			newPath: string,
			overwrite?: boolean
		): Promise<{ path?: string; error?: string }> {
			return request(`${base}/flows/move`, {
				method: 'POST',
				body: JSON.stringify({ old_path: oldPath, new_path: newPath, overwrite })
			});
		},

		deleteFlow(path: string, force?: boolean): Promise<{ deleted?: boolean; error?: string }> {
			return request(`${base}/flows/delete`, {
				method: 'POST',
				body: JSON.stringify({ path, force })
			});
		},

		runFlow(path: string, args?: unknown): Promise<RunCreatedResponse> {
			return request(`${base}/flows/run/${encodePath(path)}`, {
				method: 'POST',
				body: JSON.stringify({ args })
			});
		},

		checkExpr(expr: string): Promise<{ ok: boolean; error?: string }> {
			return request(`${base}/flows/check-expr`, {
				method: 'POST',
				body: JSON.stringify({ expr })
			});
		},

		// -- Schedules (cron) -------------------------------------------------

		listSchedules(): Promise<ScheduleListItem[]> {
			return request(`${base}/schedules/list`);
		},

		getSchedule(id: string): Promise<Schedule> {
			return request(`${base}/schedules/get/${encodeURIComponent(id)}`);
		},

		createSchedule(req: ScheduleInput): Promise<{ id: string }> {
			return request(`${base}/schedules/create`, {
				method: 'POST',
				body: JSON.stringify(req)
			});
		},

		updateSchedule(id: string, req: ScheduleInput): Promise<{ id: string }> {
			return request(`${base}/schedules/${encodeURIComponent(id)}`, {
				method: 'PUT',
				body: JSON.stringify(req)
			});
		},

		deleteSchedule(id: string): Promise<{ deleted?: boolean }> {
			return request(`${base}/schedules/${encodeURIComponent(id)}`, { method: 'DELETE' });
		},

		setScheduleEnabled(id: string, enabled: boolean): Promise<{ enabled: boolean }> {
			return request(`${base}/schedules/${encodeURIComponent(id)}/enable`, {
				method: 'POST',
				body: JSON.stringify({ enabled })
			});
		},

		runScheduleNow(id: string): Promise<RunCreatedResponse> {
			return request(`${base}/schedules/${encodeURIComponent(id)}/run`, { method: 'POST' });
		},

		previewSchedule(
			cronExpr: string,
			timezone?: string,
			count?: number
		): Promise<{ next: string[] }> {
			return request(`${base}/schedules/preview`, {
				method: 'POST',
				body: JSON.stringify({ cron_expr: cronExpr, timezone, count })
			});
		},

			// -- Secrets ----------------------------------------------------------

			listSecrets(): Promise<SecretListItem[]> {
				return request(`${base}/secrets`);
			},

			createSecret(req: SecretInput): Promise<{ path: string }> {
				return request(`${base}/secrets`, {
					method: 'POST',
					body: JSON.stringify(req)
				});
			},

			rotateSecret(path: string, req: SecretRotateInput): Promise<void> {
				return request(`${base}/secrets/${encodePath(path)}`, {
					method: 'PUT',
					body: JSON.stringify(req)
				});
			},

			deleteSecret(path: string): Promise<void> {
				return request(`${base}/secrets/${encodePath(path)}`, { method: 'DELETE' });
			},

		// -- Triggers ---------------------------------------------------------

			listTriggers(flowId: string): Promise<Trigger[]> {
				return request(`${base}/flows/${encodeURIComponent(flowId)}/triggers`);
			},

			createTrigger(flowId: string, req: TriggerInput): Promise<Trigger> {
				return request(`${base}/flows/${encodeURIComponent(flowId)}/triggers`, {
					method: 'POST',
					body: JSON.stringify(req)
				});
			},

			updateTrigger(id: string, req: TriggerUpdate): Promise<void> {
				return request(`${base}/triggers/${encodeURIComponent(id)}`, {
					method: 'PUT',
					body: JSON.stringify(req)
				});
			},

			deleteTrigger(id: string): Promise<void> {
				return request(`${base}/triggers/${encodeURIComponent(id)}`, { method: 'DELETE' });
			},

		// -- Runs -------------------------------------------------------------

		createRun(req: CreateRunRequest): Promise<RunCreatedResponse> {
			return request(`${base}/runs/create`, {
				method: 'POST',
				body: JSON.stringify(req)
			});
		},

		runWaitResult(req: CreateRunRequest, timeout?: number): Promise<RunResponse> {
			return request(`${base}/runs/run_wait_result${buildQuery({ timeout })}`, {
				method: 'POST',
				body: JSON.stringify(req)
			});
		},

		listRuns(query?: {
			status?: string;
			kind?: string;
			script_path?: string;
			created_by?: string;
			created_after_ms?: number;
			created_before_ms?: number;
			schedule_id?: string;
			trigger_id?: string;
			sort?: string;
			order?: 'asc' | 'desc';
			limit?: number;
			offset?: number;
		}): Promise<RunListItem[]> {
			return request(`${base}/runs/list${buildQuery(query ?? {})}`);
		},

		getRun(runId: string): Promise<RunResponse> {
			return request(`${base}/runs/get/${runId}`);
		},

		getRunLogs(
			runId: string,
			query?: { level?: LogLevelName; after_chunk?: number; limit?: number }
		): Promise<RunLogsResponse> {
			return request(`${base}/runs/${runId}/logs${buildQuery(query ?? {})}`);
		},

		cancelRun(runId: string, req?: CancelRunRequest): Promise<{ outcome: string }> {
			return request(`${base}/runs/${runId}/cancel`, {
				method: 'POST',
				body: JSON.stringify(req ?? {})
			});
		},

		rerun(runId: string, req?: RerunRequest): Promise<RunCreatedResponse> {
			return request(`${base}/runs/${runId}/rerun`, {
				method: 'POST',
				body: JSON.stringify(req ?? {})
			});
		},

		markSuccess(runId: string, req?: MarkRunRequest): Promise<void> {
			return request(`${base}/runs/${runId}/mark-success`, {
				method: 'POST',
				body: JSON.stringify(req ?? {})
			});
		},

		markFail(runId: string, req?: MarkRunRequest): Promise<void> {
			return request(`${base}/runs/${runId}/mark-fail`, {
				method: 'POST',
				body: JSON.stringify(req ?? {})
			});
		},

		// -- Service Logs -----------------------------------------------------

		getServiceLogs(query?: {
			service?: string;
			instance?: string;
			level?: LogLevelName;
			after_chunk?: number;
			limit?: number;
			since_ms?: number;
		}): Promise<ServiceLogsResponse> {
			return request(`${base}/services/logs${buildQuery(query ?? {})}`);
		},

		// -- Teams ------------------------------------------------------------

		listTeams(): Promise<TeamListResponse> {
			return request(`${base}/teams/list`);
		},

		getTeam(name: string): Promise<TeamListItem> {
			return request(`${base}/teams/get/${encodeURIComponent(name)}`);
		},

		createTeam(name: string, summary?: string): Promise<void> {
			return request(`${base}/teams/create`, {
				method: 'POST',
				body: JSON.stringify({ name, summary })
			});
		},

		deleteTeam(name: string): Promise<void> {
			return request(`${base}/teams/delete/${encodeURIComponent(name)}`, {
				method: 'DELETE'
			});
		},

		listTeamMembers(name: string): Promise<TeamMember[]> {
			return request(`${base}/teams/${encodeURIComponent(name)}/members`);
		},

		addTeamMember(name: string, email: string, role: 'reader' | 'writer'): Promise<void> {
			return request(`${base}/teams/${encodeURIComponent(name)}/members`, {
				method: 'POST',
				body: JSON.stringify({ email, role })
			});
		},

		updateTeamMemberRole(name: string, email: string, role: 'reader' | 'writer'): Promise<void> {
			return request(
				`${base}/teams/${encodeURIComponent(name)}/members/${encodeURIComponent(email)}/role`,
				{ method: 'PUT', body: JSON.stringify({ role }) }
			);
		},

		removeTeamMember(name: string, email: string): Promise<void> {
			return request(
				`${base}/teams/${encodeURIComponent(name)}/members/${encodeURIComponent(email)}`,
				{ method: 'DELETE' }
			);
		},

		getTeamQuota(name: string): Promise<TeamQuota> {
			return request(`${base}/teams/${encodeURIComponent(name)}/quota`);
		},

		updateTeamQuota(name: string, quota: TeamQuota): Promise<void> {
			return request(`${base}/teams/${encodeURIComponent(name)}/quota`, {
				method: 'PUT',
				body: JSON.stringify(quota)
			});
		},

		// -- Members ----------------------------------------------------------

		listMembers(): Promise<WorkspaceMember[]> {
			return request(`${base}/members`);
		},

		addMember(email: string, role: WorkspaceRole): Promise<void> {
			return request(`${base}/members`, {
				method: 'POST',
				body: JSON.stringify({ email, role })
			});
		},

		updateMemberRole(email: string, role: WorkspaceRole): Promise<void> {
			return request(`${base}/members/${encodeURIComponent(email)}`, {
				method: 'PUT',
				body: JSON.stringify({ role })
			});
		},

		removeMember(email: string): Promise<void> {
			return request(`${base}/members/${encodeURIComponent(email)}`, {
				method: 'DELETE'
			});
		},

		// -- User -------------------------------------------------------------

		getMe(): Promise<UserInfo> {
			return request(`${base}/me`);
		}
	};
}

// API surface used by components. Use `api.forWorkspace(id)` for all
// workspace-scoped calls — it pins every request to an explicit workspace ID,
// preventing stale-workspace bugs.
export const api = {
	health(): Promise<{ status: string }> {
		return request('/health', { auth: false });
	},

	listWorkspaces(): Promise<WorkspaceInfo[]> {
		return request('/api/workspaces');
	},

	searchUsers(query: string, signal?: AbortSignal): Promise<UserSearchItem[]> {
		return request(`/api/users/search${buildQuery({ q: query })}`, { signal });
	},

	// Cluster dashboard — cross-workspace, instance-admin only. Not workspace
	// scoped, so these live on the global api object rather than WorkspaceApi.
	cluster: {
		summary(): Promise<ClusterSummary> {
			return request('/api/admin/cluster/summary');
		},
		workers(): Promise<ClusterWorker[]> {
			return request('/api/admin/cluster/workers');
		},
		workerRuns(worker: string): Promise<ClusterWorkerRunsResponse> {
			return request(`/api/admin/cluster/workers/${encodeURIComponent(worker)}/runs`);
		}
	},

	// Personal API tokens — account-global (manage your own), not workspace scoped.
	tokens: {
		list(): Promise<ApiTokenListItem[]> {
			return request('/api/account/tokens');
		},
		create(name: string, expiresAt?: string): Promise<ApiTokenCreated> {
			return request('/api/account/tokens', {
				method: 'POST',
				body: JSON.stringify({ name, expires_at: expiresAt })
			});
		},
		reveal(id: string): Promise<{ token: string }> {
			return request(`/api/account/tokens/${encodeURIComponent(id)}/reveal`);
		},
		revoke(id: string): Promise<void> {
			return request(`/api/account/tokens/${encodeURIComponent(id)}`, { method: 'DELETE' });
		}
	},

	/** Create a WorkspaceApi instance pinned to a specific workspace ID. */
	forWorkspace(workspaceId: string): WorkspaceApi {
		return createWorkspaceApi(workspaceId);
	}
};
