import type { LogChunk, RunResultEvent, ServiceLogChunk, ServiceLogChunkRow } from '$lib/types';
import { api, type LogLevelName } from '$lib/services/api';

const POLL_INTERVAL_MS = 1_000;
const ERROR_BACKOFF_INITIAL_MS = 1_000;
const ERROR_BACKOFF_MAX_MS = 10_000;
/** After the run is marked completed, keep polling until this many empty
 * responses in a row. Why 6: DbLogLayer flushes every 500ms (db_log.rs),
 * so 6 × 1000ms = 6s comfortably covers the worst-case post-completion
 * flush we have observed (~2.5s). */
const DRAIN_EMPTY_THRESHOLD = 6;

export interface PollRunLogsOptions {
	level?: LogLevelName;
	after_chunk?: number;
	onLog: (chunk: LogChunk) => void;
	onResult?: (data: RunResultEvent) => void;
	/**
	 * Fires on the first successful poll, and again after every recovery
	 * from an error state. Consumers can rely on this to clear an "error"
	 * banner without needing a separate `onReconnect` event.
	 */
	onOpen?: () => void;
	onError?: (msg: string) => void;
	onClose?: () => void;
}

/**
 * Poll run logs at a fixed interval and stop after the run is completed
 * and the chunk stream has drained.
 *
 * We use polling instead of SSE because the SSE handler races with
 * DbLogLayer's async flush: the worker can mark a run completed several
 * seconds before its last log chunk lands in `run_log`, and the SSE stream
 * closes on completion before that chunk arrives. Polling continues past
 * completion for a fixed drain window.
 *
 * Returns a cleanup function that stops the poll loop. After it is called,
 * no further callbacks fire — including for any HTTP request already in
 * flight at the time of cleanup.
 */
export function pollRunLogs(
	workspaceId: string,
	runId: string,
	options: PollRunLogsOptions
): () => void {
	const ws = api.forWorkspace(workspaceId);

	let cursor = options.after_chunk;
	let stopped = false;
	let openedFired = false;
	let inErrorState = false;
	let consecutiveEmpty = 0;
	let errorBackoffMs = ERROR_BACKOFF_INITIAL_MS;

	const safeCall = <Args extends unknown[]>(
		name: string,
		fn: ((...args: Args) => void) | undefined,
		...args: Args
	): void => {
		if (!fn) return;
		try {
			fn(...args);
		} catch (e) {
			console.warn(`pollRunLogs ${name} callback threw`, e);
		}
	};

	(async () => {
		while (!stopped) {
			let response;
			try {
				response = await ws.getRunLogs(runId, {
					level: options.level,
					after_chunk: cursor
				});
			} catch (err) {
				// Cleanup may have fired during the in-flight request; drop the result.
				if (stopped) break;
				inErrorState = true;
				safeCall('onError', options.onError, err instanceof Error ? err.message : String(err));
				await sleep(errorBackoffMs);
				errorBackoffMs = Math.min(errorBackoffMs * 2, ERROR_BACKOFF_MAX_MS);
				continue;
			}

			// Caller has unmounted / switched runId since we kicked off the request.
			// Skip every callback for this iteration so the new viewer is not
			// polluted with the previous run's logs/result.
			if (stopped) break;

			// Reset error backoff after any successful HTTP response.
			errorBackoffMs = ERROR_BACKOFF_INITIAL_MS;

			// onOpen fires on the first ever success AND on every recovery from
			// an error state. LogViewer relies on the latter to clear the "error"
			// status banner; without it, a transient API failure would leave the
			// UI stuck on "Connection error" even after polling resumed.
			if (!openedFired || inErrorState) {
				openedFired = true;
				inErrorState = false;
				safeCall('onOpen', options.onOpen);
			}

			// Snapshot cursor before advancing — used below to detect historical runs.
			const wasAtStart = cursor === undefined;

			for (const chunk of response.chunks) {
				safeCall('onLog', options.onLog, {
					chunk_id: chunk.id,
					seq: chunk.seq,
					entries: chunk.entries
				});
				cursor = chunk.id;
			}

			if (response.completed) {
				if (wasAtStart && response.chunks.length > 0) {
					// Historical run: completed was already set when we polled for the
					// first time (cursor was undefined) and all log chunks came back in
					// that same response. DbLogLayer has had plenty of time to flush
					// everything, so skip the drain window and surface the result now.
					safeCall('onResult', options.onResult, {
						success: response.completed.success,
						result: response.completed.result
					});
					break;
				}

				if (response.chunks.length === 0) {
					consecutiveEmpty++;
				} else {
					consecutiveEmpty = 0;
				}
				if (consecutiveEmpty >= DRAIN_EMPTY_THRESHOLD) {
					safeCall('onResult', options.onResult, {
						success: response.completed.success,
						result: response.completed.result
					});
					break;
				}
			}

			await sleep(POLL_INTERVAL_MS);
		}

		if (!stopped) {
			// Reached only via the drain-completion break above; cleanup() path
			// has already torn down the consumer so we suppress onClose then.
			safeCall('onClose', options.onClose);
		}
	})();

	return () => {
		stopped = true;
	};
}

export interface PollServiceLogsOptions {
	service?: string;
	instance?: string;
	level?: LogLevelName;
	after_chunk?: number;
	/**
	 * When set, the first request fetches chunks with created_at >= sinceMs
	 * (milliseconds since Unix epoch). Subsequent requests use cursor-based
	 * polling so only new entries are returned. Pass `Date.now() - windowMs`
	 * to implement time-based tail (e.g. last 5 / 15 / 30 minutes).
	 */
	sinceMs?: number;
	onLog: (chunk: ServiceLogChunk) => void;
	/**
	 * Fires on first successful poll and after every recovery from error state.
	 */
	onOpen?: () => void;
	onError?: (msg: string) => void;
}

function rowToChunk(row: ServiceLogChunkRow): ServiceLogChunk {
	return {
		chunk_id: row.id,
		seq: row.seq,
		instance_id: row.instance_id,
		service: row.service,
		entries: row.entries
	};
}

/**
 * Poll service logs at a fixed interval. Runs indefinitely until the
 * returned cleanup function is called.
 *
 * Uses polling instead of SSE for consistency with pollRunLogs and to
 * avoid the SSE reconnection complexity for a long-lived stream.
 */
export function pollServiceLogs(
	workspaceId: string,
	options: PollServiceLogsOptions
): () => void {
	const ws = api.forWorkspace(workspaceId);

	let cursor = options.after_chunk;
	let stopped = false;
	let openedFired = false;
	let inErrorState = false;
	let errorBackoffMs = ERROR_BACKOFF_INITIAL_MS;
	// Idle backoff: stretch the interval when there are no new chunks so we
	// don't hammer the server during quiet periods. Resets immediately on
	// any non-empty response so live bursts are still delivered at 1 s.
	let idleIntervalMs = POLL_INTERVAL_MS;
	// True only for the very first request when time-based tail is requested.
	let sinceInit = options.sinceMs !== undefined && cursor === undefined;

	const safeCall = <Args extends unknown[]>(
		name: string,
		fn: ((...args: Args) => void) | undefined,
		...args: Args
	): void => {
		if (!fn) return;
		try {
			fn(...args);
		} catch (e) {
			console.warn(`pollServiceLogs ${name} callback threw`, e);
		}
	};

	(async () => {
		while (!stopped) {
			let response;
			// Snapshot sinceInit before the await. On success we clear it so
			// subsequent polls use cursor-based mode. On error it stays set so
			// the retry re-attempts the since fetch rather than falling back to
			// after_chunk=0 (which would load all history).
			const useSinceInit = sinceInit;
			try {
				if (useSinceInit) {
					response = await ws.getServiceLogs({
						service: options.service,
						instance: options.instance,
						level: options.level,
						since_ms: options.sinceMs,
						limit: 200
					});
					// Only clear sinceInit when we got data (and thus a cursor to
					// continue from). If the window was empty, keep using since_ms
					// so we don't fall through to after_chunk=undefined which the
					// backend resolves to 0 (full history).
					if (response.next_cursor !== null) {
						sinceInit = false;
					}
				} else {
					response = await ws.getServiceLogs({
						service: options.service,
						instance: options.instance,
						level: options.level,
						after_chunk: cursor,
						limit: 200
					});
				}
			} catch (err) {
				if (stopped) break;
				inErrorState = true;
				idleIntervalMs = POLL_INTERVAL_MS;
				safeCall('onError', options.onError, err instanceof Error ? err.message : String(err));
				await sleep(errorBackoffMs);
				errorBackoffMs = Math.min(errorBackoffMs * 2, ERROR_BACKOFF_MAX_MS);
				continue;
			}

			if (stopped) break;

			errorBackoffMs = ERROR_BACKOFF_INITIAL_MS;

			if (!openedFired || inErrorState) {
				openedFired = true;
				inErrorState = false;
				safeCall('onOpen', options.onOpen);
			}

			for (const row of response.chunks) {
				safeCall('onLog', options.onLog, rowToChunk(row));
			}

			if (response.next_cursor !== null) {
				cursor = response.next_cursor;
			}

			if (response.chunks.length > 0) {
				// Got data — reset to base interval for fast follow-up.
				idleIntervalMs = POLL_INTERVAL_MS;
			} else {
				// No new chunks — back off up to 10 s to reduce idle traffic.
				idleIntervalMs = Math.min(idleIntervalMs * 2, ERROR_BACKOFF_MAX_MS);
			}

			await sleep(idleIntervalMs);
		}
	})();

	return () => {
		stopped = true;
	};
}

function sleep(ms: number): Promise<void> {
	return new Promise((resolve) => setTimeout(resolve, ms));
}
