/**
 * Render an ISO 8601 timestamp as a short relative-time string suitable
 * for compact UI surfaces (history rows, diff headers, tooltips).
 *
 * Examples:
 *   - "just now" (< 1 min)
 *   - "5m ago", "23m ago"
 *   - "3h ago"
 *   - "2d ago"
 *   - "2024-08-13"   (more than 30 days)
 *
 * Returns the original string unchanged if it cannot be parsed.
 */
export function formatRelative(iso: string): string {
	const d = new Date(iso);
	if (Number.isNaN(d.getTime())) return iso;
	const diffMs = Date.now() - d.getTime();
	const min = Math.floor(diffMs / 60_000);
	if (min < 1) return 'just now';
	if (min < 60) return `${min}m ago`;
	const hr = Math.floor(min / 60);
	if (hr < 24) return `${hr}h ago`;
	const day = Math.floor(hr / 24);
	if (day < 30) return `${day}d ago`;
	return d.toISOString().slice(0, 10);
}

/** Numeric UTC-offset label for `d` in `timeZone` (undefined = browser zone):
 *  `UTC+8`, `UTC+0`, `UTC-4`, `UTC+5:30`. */
function zoneOffsetLabel(d: Date, timeZone: string | undefined): string {
	const name =
		new Intl.DateTimeFormat('en-US', { timeZone, timeZoneName: 'longOffset' })
			.formatToParts(d)
			.find((p) => p.type === 'timeZoneName')?.value ?? 'GMT+00:00';
	const m = name.match(/GMT([+-])(\d{2}):?(\d{2})?/);
	if (!m) return 'UTC';
	const sign = m[1];
	const h = parseInt(m[2], 10);
	const mm = m[3] ? parseInt(m[3], 10) : 0;
	if (h === 0 && mm === 0) return 'UTC';
	return mm ? `UTC${sign}${h}:${String(mm).padStart(2, '0')}` : `UTC${sign}${h}`;
}

/**
 * Render an ISO 8601 timestamp as an absolute date+time in a chosen timezone.
 * `tz` is 'Local' (browser zone) or an IANA name (e.g. 'Asia/Taipei'); pass
 * `displayTz.value` from the timezone store. A numeric offset label (e.g.
 * `UTC+8`) is appended so the displayed time is unambiguous.
 */
export function formatAbsolute(iso: string | null | undefined, tz: string): string {
	if (!iso) return '—';
	const d = new Date(iso);
	if (Number.isNaN(d.getTime())) return iso;
	const timeZone = tz && tz !== 'Local' ? tz : undefined;
	try {
		// Fixed `sv-SE` locale → ISO-like `2026-06-10 14:36:00` (24h, no locale-specific
		// year/month words), consistent with the log viewer and unambiguous regardless of
		// the browser locale. The zone is shown as a numeric `UTC±N` offset (appended
		// separately — `timeZoneName` can't be combined with these component options).
		// Seconds included (sub-minute cron supports 10s intervals).
		const time = new Intl.DateTimeFormat('sv-SE', {
			year: 'numeric',
			month: '2-digit',
			day: '2-digit',
			hour: '2-digit',
			minute: '2-digit',
			second: '2-digit',
			hour12: false,
			timeZone
		}).format(d);
		return `${time} ${zoneOffsetLabel(d, timeZone)}`;
	} catch {
		// Invalid timeZone (shouldn't happen with curated options) → browser local.
		return d.toLocaleString();
	}
}
