export const DEFAULT_LOG_CAP = 10_000;

/**
 * Append one entry to a buffer, returning a new array capped at `cap`.
 * Pure: never mutates the input. The oldest entries are dropped from the front
 * when the cap is exceeded.
 *
 * Generic so the same helper covers run logs and (future) service logs without
 * an `as` cast inside.
 */
export function appendCapped<T>(
	arr: readonly T[],
	entry: T,
	cap = DEFAULT_LOG_CAP
): readonly T[] {
	const next = arr.concat(entry);
	return next.length > cap ? next.slice(next.length - cap) : next;
}

/**
 * Append many entries to a buffer, returning a new array capped at `cap`.
 * Pure: never mutates the input.
 *
 * Empty chunk: returns the input ref unchanged. Combined with `$state.raw`
 * on the consumer side, this is a no-op reassignment that intentionally skips
 * a render.
 */
export function appendManyCapped<T>(
	arr: readonly T[],
	entries: readonly T[],
	cap = DEFAULT_LOG_CAP
): readonly T[] {
	if (entries.length === 0) return arr;
	const next = arr.concat(entries as T[]);
	return next.length > cap ? next.slice(next.length - cap) : next;
}
