// Display timezone preference (Airflow-style UI tz picker). Display-only: all
// timestamps are stored/transmitted in UTC; this just changes how they render.
// Distinct from a schedule's own `timezone` (which controls when cron fires).
const KEY = 'display-tz';

// 'Local' = the browser's timezone; otherwise an IANA name. Default UTC so a
// team sees consistent times regardless of each person's machine.
function load(): string {
	try {
		return localStorage.getItem(KEY) || 'UTC';
	} catch {
		return 'UTC';
	}
}

let tz = $state(load());

export const displayTz = {
	/** Current preference: 'Local' or an IANA zone. */
	get value(): string {
		return tz;
	},
	set(next: string): void {
		tz = next;
		try {
			localStorage.setItem(KEY, next);
		} catch {
			/* localStorage unavailable — keep in-memory only */
		}
	}
};

/** Picker options: just UTC and the browser's local timezone. */
export const TZ_OPTIONS: string[] = ['UTC', 'Local'];
