import { api, ApiClientError, type WorkspaceApi } from '$lib/services/api';
import { workspace } from '$lib/stores/workspace.svelte';

interface WorkspaceLoaderOptions {
	/** Additional reactive dependency — loader re-runs when its return value changes. */
	key?: () => string;
	/** Gate: loader only runs when this returns true. */
	enabled?: () => boolean;
	/**
	 * Keep the last successfully loaded data visible while re-fetching for the
	 * same workspace (reload or key change), instead of blanking it to `null`.
	 * Lets consumers keep a heavy child mounted across a reload — e.g. the run
	 * page's Monaco editor, which would otherwise be torn down and recreated on
	 * every refresh. Data is still cleared when the workspace itself changes.
	 */
	keepData?: boolean;
}

interface WorkspaceLoaderResult<T> {
	readonly data: T | null;
	readonly loading: boolean;
	readonly error: string;
	reload: () => void;
}

/**
 * Reactive workspace-scoped data loader with built-in stale-guard.
 *
 * Re-runs automatically when `workspace.id` changes (and optionally when
 * `options.key` changes). Discards responses that arrive after the workspace
 * has already switched, preventing stale data from leaking into the UI.
 *
 * Also guards against same-workspace request races (e.g. rapid Refresh clicks):
 * each call to `execute` increments a sequence counter and only the latest
 * in-flight request is allowed to commit its result.
 *
 * Usage:
 *   const scripts = useWorkspaceLoader((ws) => ws.listScripts());
 *   // template: {scripts.data}, {scripts.loading}, {scripts.error}
 */
export function useWorkspaceLoader<T>(
	loader: (wsApi: WorkspaceApi, wsId: string) => Promise<T>,
	options?: WorkspaceLoaderOptions
): WorkspaceLoaderResult<T> {
	let data: T | null = $state(null);
	let loading = $state(true);
	let error = $state('');

	// Monotonically increasing counter to detect stale responses from
	// same-workspace request races (e.g. double Refresh click).
	let seq = 0;
	// Workspace + key whose data is currently held, so keepData can tell a same-key
	// reload (retain) from a workspace switch or key change (blank — never show one
	// entity's data under another's, e.g. parent flow under a child run's URL).
	let loadedWsId: string | null = null;
	let loadedKey: string | undefined;

	async function execute(wsId: string) {
		const thisSeq = ++seq;
		const key = options?.key?.();
		// Blank stale data before fetching, unless the caller opted to keep it for a
		// same-workspace, same-key reload. A workspace switch or key change always
		// blanks regardless (otherwise the old entity flashes under the new URL).
		if (!options?.keepData || wsId !== loadedWsId || key !== loadedKey) {
			data = null;
		}
		loading = true;
		error = '';

		try {
			const result = await loader(api.forWorkspace(wsId), wsId);
			// Stale guard: workspace drift OR a newer request superseded this one.
			if (workspace.id !== wsId || thisSeq !== seq) return;
			data = result;
			loadedWsId = wsId;
			loadedKey = key;
		} catch (e) {
			if (workspace.id !== wsId || thisSeq !== seq) return;
			if (e instanceof ApiClientError) {
				error = `${e.status}: ${e.body || e.message}`;
			} else {
				error = e instanceof Error ? e.message : 'Request failed';
			}
		} finally {
			if (workspace.id === wsId && thisSeq === seq) {
				loading = false;
			}
		}
	}

	$effect(() => {
		const wsId = workspace.id;
		// Read optional reactive dependencies so Svelte tracks them.
		const _key = options?.key?.();
		const enabled = options?.enabled?.() ?? true;

		// Suppress to avoid unused-variable lint — the values are read for reactivity.
		void _key;

		if (wsId && enabled) {
			void execute(wsId);
		}
	});

	return {
		get data() {
			return data;
		},
		get loading() {
			return loading;
		},
		get error() {
			return error;
		},
		reload() {
			const wsId = workspace.id;
			if (wsId) {
				void execute(wsId);
			}
		}
	};
}
