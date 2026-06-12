/**
 * Render a millisecond duration as a short human-readable string.
 *
 * Examples:
 *   0       → "0ms"
 *   42      → "42ms"
 *   850     → "850ms"
 *   1_200   → "1.2s"
 *   45_700  → "45.7s"
 *   83_000  → "1m23s"
 *   3_705_000 → "1h2m"
 *
 * Returns `null` mapped to `'—'` so the caller doesn't need a separate branch.
 */
export function formatDuration(ms: number | null | undefined): string {
	if (ms === null || ms === undefined) return '—';
	if (ms < 0) return '—';
	if (ms < 1000) return `${ms}ms`;
	if (ms < 60_000) return `${(ms / 1000).toFixed(1)}s`;
	const totalSec = Math.floor(ms / 1000);
	const min = Math.floor(totalSec / 60);
	const sec = totalSec % 60;
	if (min < 60) return `${min}m${sec.toString().padStart(2, '0')}s`;
	const hr = Math.floor(min / 60);
	const remMin = min % 60;
	return `${hr}h${remMin.toString().padStart(2, '0')}m`;
}
