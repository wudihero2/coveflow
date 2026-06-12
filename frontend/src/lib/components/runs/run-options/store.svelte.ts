import type { RunOptions } from './types';

/**
 * Session-sticky Run Options store.
 *
 * Module-level $state object — persists across navigation within the SPA session
 * (per-tab). Reload or new tab resets to defaults.
 *
 * Mutate fields directly: `runOptions.timeout = 600`.
 */
export const runOptions = $state<RunOptions>({
	args: undefined,
	tag: undefined,
	timeout: undefined,
	priority: undefined,
	cpus: undefined,
	memoryMb: undefined,
	diskMb: undefined,
	teamOwner: null
});

/** Reset all options to defaults (e.g. on logout). */
export function resetRunOptions(): void {
	runOptions.args = undefined;
	runOptions.tag = undefined;
	runOptions.timeout = undefined;
	runOptions.priority = undefined;
	runOptions.cpus = undefined;
	runOptions.memoryMb = undefined;
	runOptions.diskMb = undefined;
	runOptions.teamOwner = null;
}
