// =============================================================================
// Workspace Store  (Svelte 5 runes — .svelte.ts file)
// =============================================================================
//
// Tracks which workspace the user is currently working in.
// Most API calls include `workspace_id` in the URL path, so this store
// is the single source of truth for that value.
//
// Usage:
//   import { workspace } from '$lib/stores/workspace.svelte';
//   workspace.id                   // current workspace ID
//   workspace.list                 // all workspaces the user belongs to
//   workspace.switch('other-ws')   // change workspace
//
// On login, auth.svelte.ts calls `workspace.load()` automatically.
// If the user belongs to only one workspace, it auto-selects and the UI
// can hide the workspace switcher.
// =============================================================================

import { browser } from '$app/environment';
import type { WorkspaceInfo } from '$lib/types';

// ---------------------------------------------------------------------------
// Internal reactive state
// ---------------------------------------------------------------------------

const LAST_WORKSPACE_KEY = 'coveflow:last-workspace:v1';

function readStoredWorkspaceId(): string {
	if (!browser) return 'default';

	try {
		const stored = sessionStorage.getItem(LAST_WORKSPACE_KEY);
		return stored && stored.trim() ? stored : 'default';
	} catch {
		return 'default';
	}
}

function writeStoredWorkspaceId(id: string): void {
	if (!browser) return;

	try {
		sessionStorage.setItem(LAST_WORKSPACE_KEY, id);
	} catch {
		// Storage can be unavailable in private or restricted browser contexts.
	}
}

const initialWorkspaceId = readStoredWorkspaceId();

let currentId = $state<string>(initialWorkspaceId);
let lastSelectedId = $state<string>(initialWorkspaceId);
let workspaceList = $state<WorkspaceInfo[]>([]);

/** Default fallback when the backend doesn't have a workspace list endpoint yet. */
const DEFAULT_WORKSPACE: WorkspaceInfo = { id: 'default', name: 'Default', owner: '' };

function rememberWorkspace(id: string): void {
	currentId = id;
	lastSelectedId = id;
	writeStoredWorkspaceId(id);
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

interface WorkspaceStore {
	readonly id: string;
	readonly lastId: string;
	readonly list: WorkspaceInfo[];
	readonly showSwitcher: boolean;
	load(token: string): Promise<void>;
	switch(id: string): void;
	reset(): void;
}

export const workspace: WorkspaceStore = {
	// -- Reactive getters ----------------------------------------------------

	/** The currently selected workspace ID (used in API URL paths). */
	get id(): string {
		return currentId;
	},

	/** Last selected workspace ID, preserved across transient auth resets. */
	get lastId(): string {
		return lastSelectedId;
	},

	/** All workspaces the authenticated user belongs to. */
	get list(): WorkspaceInfo[] {
		return workspaceList;
	},

	/**
	 * Whether the sidebar should show a workspace switcher dropdown.
	 * Only needed when the user belongs to more than one workspace.
	 */
	get showSwitcher(): boolean {
		return workspaceList.length > 1;
	},

	// -- Actions -------------------------------------------------------------

	/**
	 * Fetch the user's workspace list from the backend and auto-select.
	 *
	 * Called by auth.svelte.ts after successful login/signup/refresh.
	 * It sets `currentId` to the first workspace (or keeps the current
	 * one if it's still valid).
	 *
	 * Error handling:
	 *   - 404: endpoint not implemented yet → fall back to 'default'
	 *   - 401/403: auth problem → re-throw so the caller (auth store) can react
	 *   - Network error → re-throw
	 *
	 * Note: uses raw fetch rather than api.ts to avoid circular dependencies.
	 */
	async load(token: string): Promise<void> {
		const res = await fetch('/api/workspaces', {
			headers: { Authorization: `Bearer ${token}` }
		});

		if (res.status === 404) {
			workspaceList = [DEFAULT_WORKSPACE];
			rememberWorkspace('default');
			return;
		}

		if (!res.ok) {
			// 401/403/500 etc — something is actually wrong.
			// Re-throw so auth.handleAuthSuccess can decide what to do
			// (e.g. clear session on 401).
			const text = await res.text();
			throw new Error(`Failed to load workspaces: ${res.status} ${text}`);
		}

		const data: WorkspaceInfo[] = await res.json();

		if (data.length === 0) {
			// User exists but has no workspace — likely a data inconsistency
			// since personal workspace provisioning should guarantee at least one.
			throw new Error('No workspaces found for user. This may indicate a data inconsistency.');
		}

		workspaceList = data;

		// Keep current selection if it's still in the list
		const stillValid = data.some((w) => w.id === currentId);
		if (!stillValid) {
			rememberWorkspace(data[0].id);
		} else {
			rememberWorkspace(currentId);
		}
	},

	/**
	 * Switch to a different workspace.
	 * The UI should reload page data after calling this (e.g. invalidateAll).
	 */
	switch(id: string): void {
		if (workspaceList.some((w) => w.id === id) && id !== currentId) {
			rememberWorkspace(id);
		}
	},

	/**
	 * Reset to defaults (called by auth.logout / auth.tryRefresh on failure).
	 */
	reset(): void {
		currentId = 'default';
		workspaceList = [];
	}
};
