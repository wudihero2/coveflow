import type { LogChunk, RunResultEvent, ServiceLogChunk } from '$lib/types';
import type { LogLevelName } from '$lib/services/api';
import { buildQuery } from '$lib/services/url';
import { auth, tryRefresh } from '$lib/stores/auth.svelte';

// -----------------------------------------------------------------------------
// Internal SSE parser
// -----------------------------------------------------------------------------

interface SSEEvent {
	event: string;
	data: string;
}

/**
 * Parse a raw SSE text buffer into discrete events.
 * Events are separated by blank lines (`\n\n`).
 * Returns [parsed events, remaining incomplete buffer].
 */
function parseSSE(buffer: string): [SSEEvent[], string] {
	const events: SSEEvent[] = [];
	const blocks = buffer.split('\n\n');

	// Last element may be an incomplete block — keep it in the buffer.
	const remainder = blocks.pop() ?? '';

	for (const block of blocks) {
		if (!block.trim()) continue;

		let event = '';
		let data = '';

		for (const line of block.split('\n')) {
			if (line.startsWith('event:')) {
				event = line.slice(6).trim();
			} else if (line.startsWith('data:')) {
				data += (data ? '\n' : '') + line.slice(5).trim();
			}
			// Ignore id:, retry:, comments (:)
		}

		if (data) {
			events.push({ event: event || 'message', data });
		}
	}

	return [events, remainder];
}

// -----------------------------------------------------------------------------
// Core streaming helper with reconnection
// -----------------------------------------------------------------------------

const MAX_RETRIES = 10;
const INITIAL_BACKOFF_MS = 500;
const MAX_BACKOFF_MS = 30_000;

interface StreamOptions {
	/** Build the URL for (re)connection. Called on each attempt so cursor can advance. */
	buildUrl: () => string;
	/** Callback for each parsed SSE event. Return true to signal "stream is done, stop reconnecting". */
	onEvent: (event: SSEEvent) => boolean;
	/** Called once per successful HTTP connection (before any events). Use this for "streaming"
	 * UI states that should activate as soon as the connection is alive — independent of whether
	 * the backend has sent any data yet. Fires again on each successful reconnect. */
	onOpen?: () => void;
	/** Called on errors (network or parse). The stream still attempts to reconnect unless aborted. */
	onError?: (msg: string) => void;
	/** Called when the stream ends and will NOT reconnect (either done or max retries). */
	onClose?: () => void;
}

/**
 * Open an SSE stream with automatic reconnection and exponential backoff.
 *
 * Uses fetch + ReadableStream (not EventSource) because SSE needs
 * the Authorization header.
 *
 * Returns a cleanup function that aborts the connection and stops reconnection.
 */
function openStream(options: StreamOptions): () => void {
	const controller = new AbortController();
	let retries = 0;

	(async () => {
		while (!controller.signal.aborted) {
			try {
				const headers: Record<string, string> = {
					Accept: 'text/event-stream'
				};

				if (auth.token) {
					headers['Authorization'] = `Bearer ${auth.token}`;
				}

				const url = options.buildUrl();
				let response = await fetch(url, {
					headers,
					signal: controller.signal
				});

				// If the token expired, try refreshing once.
				if (response.status === 401 && (await tryRefresh())) {
					headers['Authorization'] = `Bearer ${auth.token}`;
					response = await fetch(url, {
						headers,
						signal: controller.signal
					});
				}

				if (!response.ok) {
					const body = await response.text();
					options.onError?.(`SSE request failed: ${response.status} ${body}`);
					// Non-retryable HTTP errors (4xx).
					if (response.status >= 400 && response.status < 500) break;
					// Server errors — fall through to retry logic below.
					throw new Error(`HTTP ${response.status}`);
				}

				if (!response.body) {
					options.onError?.('SSE response has no body');
					break;
				}

				// Connected successfully — reset retry counter.
				retries = 0;
				// Defend the reader path from a callback that throws — the helper's
				// own correctness must not depend on onOpen being side-effect-safe.
				// TODO(observability): route this through telemetry/Sentry once
				// the frontend pipeline lands; console.warn is the interim sink.
				try {
					options.onOpen?.();
				} catch (e) {
					console.warn('SSE onOpen callback threw', e);
				}

				const reader = response.body.getReader();
				const decoder = new TextDecoder();
				let buffer = '';
				let done = false;

				for (;;) {
					const result = await reader.read();
					if (result.done) break;

					buffer += decoder.decode(result.value, { stream: true });
					const [events, remainder] = parseSSE(buffer);
					buffer = remainder;

					for (const event of events) {
						if (options.onEvent(event)) {
							done = true;
							break;
						}
					}
					if (done) break;
				}

				if (done) {
					// Stream signalled completion (e.g. "result" event for run logs).
					options.onClose?.();
					return;
				}

				// Stream ended without a "done" signal — reconnect.
			} catch (err: unknown) {
				if (err instanceof DOMException && err.name === 'AbortError') {
					return;
				}
				options.onError?.(err instanceof Error ? err.message : String(err));
			}

			// Exponential backoff before reconnecting.
			retries++;
			if (retries > MAX_RETRIES) {
				options.onError?.(`SSE gave up after ${MAX_RETRIES} retries`);
				break;
			}
			const delay = Math.min(INITIAL_BACKOFF_MS * 2 ** (retries - 1), MAX_BACKOFF_MS);
			await sleep(delay, controller.signal);
		}

		options.onClose?.();
	})();

	return () => controller.abort();
}

function sleep(ms: number, signal: AbortSignal): Promise<void> {
	return new Promise((resolve) => {
		const timer = setTimeout(resolve, ms);
		signal.addEventListener('abort', () => {
			clearTimeout(timer);
			resolve();
		}, { once: true });
	});
}

// -----------------------------------------------------------------------------
// Public streaming APIs
// -----------------------------------------------------------------------------

export interface StreamRunLogsOptions {
	level?: LogLevelName;
	after_chunk?: number;
	onLog: (chunk: LogChunk) => void;
	onResult?: (data: RunResultEvent) => void;
	/** Fired on every successful HTTP connect (including reconnects). */
	onOpen?: () => void;
	onError?: (msg: string) => void;
	onClose?: () => void;
}

/**
 * Stream run logs via SSE with automatic reconnection.
 *
 * SSE events:
 *   "log"    → LogChunk  (updates the cursor for reconnection)
 *   "result" → RunResultEvent  (run completed — stops reconnecting)
 *
 * Returns a cleanup function to abort the connection.
 */
export function streamRunLogs(
	workspaceId: string,
	runId: string,
	options: StreamRunLogsOptions
): () => void {
	let cursor = options.after_chunk;

	return openStream({
		buildUrl() {
			const params: Record<string, string | number | undefined> = {
				level: options.level,
				after_chunk: cursor
			};
			return `/api/workspaces/${workspaceId}/runs/${runId}/logs/stream${buildQuery(params)}`;
		},
		onEvent(event) {
			try {
				if (event.event === 'error') {
					options.onError?.(event.data);
					return true; // Server-side error — stop reconnecting.
				}
				if (event.event === 'log') {
					const chunk = JSON.parse(event.data) as LogChunk;
					cursor = chunk.chunk_id;
					options.onLog(chunk);
				} else if (event.event === 'result') {
					options.onResult?.(JSON.parse(event.data) as RunResultEvent);
					return true; // Done — stop reconnecting.
				}
			} catch {
				options.onError?.(`Failed to parse SSE data: ${event.data}`);
			}
			return false;
		},
		onOpen: options.onOpen,
		onError: options.onError,
		onClose: options.onClose
	});
}

export interface StreamServiceLogsOptions {
	service?: string;
	instance?: string;
	level?: LogLevelName;
	after_chunk?: number;
	onLog: (chunk: ServiceLogChunk) => void;
	/** Fired on every successful HTTP connect (including reconnects). */
	onOpen?: () => void;
	onError?: (msg: string) => void;
	onClose?: () => void;
}

/**
 * Stream service logs via SSE with automatic reconnection.
 *
 * SSE events:
 *   "log" → ServiceLogChunk
 *
 * This stream runs indefinitely until the caller aborts it.
 * On disconnect it automatically reconnects from the last received chunk.
 * Returns a cleanup function to abort the connection.
 */
export function streamServiceLogs(
	workspaceId: string,
	options: StreamServiceLogsOptions
): () => void {
	let cursor = options.after_chunk;

	return openStream({
		buildUrl() {
			const params: Record<string, string | number | undefined> = {
				service: options.service,
				instance: options.instance,
				level: options.level,
				after_chunk: cursor
			};
			return `/api/workspaces/${workspaceId}/services/logs/stream${buildQuery(params)}`;
		},
		onEvent(event) {
			try {
				if (event.event === 'error') {
					options.onError?.(event.data);
					return true; // Server-side error — stop reconnecting.
				}
				if (event.event === 'log') {
					const chunk = JSON.parse(event.data) as ServiceLogChunk;
					cursor = chunk.chunk_id;
					options.onLog(chunk);
				}
			} catch {
				options.onError?.(`Failed to parse SSE data: ${event.data}`);
			}
			return false; // Never "done" — keep reconnecting.
		},
		onOpen: options.onOpen,
		onError: options.onError,
		onClose: options.onClose
	});
}
