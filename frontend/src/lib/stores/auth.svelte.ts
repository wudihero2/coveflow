// =============================================================================
// Auth Store  (Svelte 5 runes — .svelte.ts file)
// =============================================================================
//
// Manages JWT authentication state.  The access token lives **only in memory**
// (never localStorage) to mitigate XSS token theft.  A refresh token is stored
// by the backend as an HttpOnly cookie, so the browser sends it automatically
// on `/api/auth/refresh` requests — JavaScript can't read it.
//
// Usage:
//   import { auth } from '$lib/stores/auth.svelte';
//   auth.login(email, password);   // POST /api/auth/login
//   auth.token                     // current JWT (or null)
//   auth.isAuthenticated           // boolean shorthand
//
// Why export an object instead of `let token = $state(null)`?
// Svelte 5 can't export a reassigned $state variable across modules.
// Wrapping in an object (`auth.token`) keeps reactivity working because
// Svelte's proxy tracks property access on the object.
// See: https://svelte.dev/docs/svelte/$state#Passing-state-across-modules
// =============================================================================

import type { AuthResponse } from '$lib/types';
import { workspace } from './workspace.svelte';

// ---------------------------------------------------------------------------
// Internal reactive state — not exported directly
// ---------------------------------------------------------------------------

let token = $state<string | null>(null);
let email = $state<string | null>(null);
let refreshTimerId = $state<ReturnType<typeof setTimeout> | null>(null);

// ---------------------------------------------------------------------------
// Refresh deduplication
// ---------------------------------------------------------------------------
//
// The backend revokes the old refresh token when issuing a new one
// If two refresh calls race (e.g. timer fires while
// a 401-retry also calls tryRefresh), the second one will fail because its
// cookie was already revoked, and we'd incorrectly clear the session.
//
// `inflightRefresh` ensures only one refresh request is in-flight at a time;
// concurrent callers share the same promise.
// ---------------------------------------------------------------------------

let inflightRefresh: Promise<boolean> | null = null;

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/**
 * Schedule a silent refresh `(expiresIn - 60)` seconds from now.
 * If the token expires in 900s (15 min), we refresh at 840s (14 min)
 * so the user never sees an expired-token error.
 */
function scheduleRefresh(expiresIn: number): void {
	clearScheduledRefresh();
	const delayMs = Math.max((expiresIn - 60) * 1000, 5_000); // at least 5s
	refreshTimerId = setTimeout(() => {
		tryRefresh();
	}, delayMs);
}

function clearScheduledRefresh(): void {
	if (refreshTimerId !== null) {
		clearTimeout(refreshTimerId);
		refreshTimerId = null;
	}
}

/**
 * Send a POST request with JSON body to an auth endpoint.
 * Returns the parsed response or throws on HTTP error.
 */
async function authPost<T>(path: string, body?: Record<string, unknown>): Promise<T> {
	const res = await fetch(path, {
		method: 'POST',
		headers: body ? { 'Content-Type': 'application/json' } : {},
		body: body ? JSON.stringify(body) : undefined,
		// Include cookies so the HttpOnly refresh_token cookie is sent
		credentials: 'same-origin'
	});

	if (!res.ok) {
		const text = await res.text();
		throw new Error(text || `${res.status} ${res.statusText}`);
	}

	return res.json() as Promise<T>;
}

/**
 * Handle a successful auth response:
 *   1. Save token + email in memory
 *   2. Schedule next refresh
 *   3. Load the user's workspace list
 *
 * Note: workspace-specific role is fetched separately by the layout,
 * not here, to avoid circular imports with api.ts.
 */
async function handleAuthSuccess(data: AuthResponse): Promise<void> {
	// Load workspaces first — if this throws, we don't commit auth state
	await workspace.load(data.access_token);
	token = data.access_token;
	email = data.email;
	scheduleRefresh(data.expires_in);
}

/**
 * Clear all session state (auth + workspace).
 */
function clearSession(): void {
	token = null;
	email = null;
	clearScheduledRefresh();
	workspace.reset();
}

// ---------------------------------------------------------------------------
// Public API — exported as a single object so $state reactivity works
// ---------------------------------------------------------------------------

export const auth = {
	// -- Reactive getters ----------------------------------------------------

	/** Current JWT access token (null if not authenticated). */
	get token(): string | null {
		return token;
	},

	/** Convenience boolean for template `{#if auth.isAuthenticated}`. */
	get isAuthenticated(): boolean {
		return token !== null;
	},

	/** Email of the currently authenticated user. */
	get email(): string | null {
		return email;
	},

	// -- Actions -------------------------------------------------------------

	/**
	 * Log in with email + password.
	 * POST /api/auth/login -> { access_token, expires_in, email }
	 * Backend also sets an HttpOnly `refresh_token` cookie.
	 * After login, automatically loads the user's workspace list.
	 */
	async login(email: string, password: string): Promise<void> {
		const data = await authPost<AuthResponse>('/api/auth/login', { email, password });
		await handleAuthSuccess(data);
	},

	/**
	 * Create a new account.
	 * POST /api/auth/signup -> { access_token, expires_in, email }
	 * After signup, automatically loads the user's workspace list.
	 */
	async signup(email: string, password: string): Promise<void> {
		const data = await authPost<AuthResponse>('/api/auth/signup', { email, password });
		await handleAuthSuccess(data);
	},

	/**
	 * Silently refresh the access token using the HttpOnly cookie.
	 * POST /api/auth/refresh -> { access_token, expires_in, email }
	 *
	 * Called automatically by the scheduled timer, but can also be
	 * called manually (e.g. on app init to restore a session after
	 * a page refresh).
	 *
	 * Concurrent calls are deduplicated: if a refresh is already in-flight,
	 * subsequent callers share the same promise.  This prevents the backend
	 * from revoking a cookie that a second request is still using.
	 *
	 * Returns `true` if refresh succeeded, `false` if it failed
	 * (e.g. cookie expired → user must log in again).
	 */
	async tryRefresh(): Promise<boolean> {
		// Deduplicate: return the in-flight promise if one exists
		if (inflightRefresh) {
			return inflightRefresh;
		}

		inflightRefresh = (async () => {
			try {
				const data = await authPost<AuthResponse>('/api/auth/refresh');
				await handleAuthSuccess(data);
				return true;
			} catch {
				// Refresh failed — clear state so the UI shows login
				clearSession();
				return false;
			} finally {
				inflightRefresh = null;
			}
		})();

		return inflightRefresh;
	},

	/**
	 * Log out: tell the backend to invalidate the session, then clear
	 * local state (both auth and workspace).
	 * POST /api/auth/logout (backend clears the refresh_token cookie).
	 */
	async logout(): Promise<void> {
		try {
			await authPost('/api/auth/logout');
		} catch {
			// Even if the request fails, clear local state
		}
		clearSession();
	}
};

// Re-export tryRefresh as a standalone function so `api.ts` can call it
// without circular-import issues (api.ts doesn't need the full `auth` object).
export const tryRefresh = auth.tryRefresh.bind(auth);
