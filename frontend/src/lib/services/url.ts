// Percent-encode each segment of a script path while preserving the '/'
// hierarchy, so a path like "folders/a b/x.py" survives both API routes and
// SvelteKit [...path] route params. Shared so the encoding stays consistent
// across the API client, route navigations, and panels.
export function encodePath(path: string): string {
	return path.split('/').map(encodeURIComponent).join('/');
}

// Build a query string from optional params, filtering out null/undefined values.
export function buildQuery(params: Record<string, string | number | undefined>): string {
	const entries = Object.entries(params).filter(
		(entry): entry is [string, string | number] => entry[1] != null
	);

	return entries.length
		? '?' + new URLSearchParams(entries.map(([key, value]) => [key, String(value)])).toString()
		: '';
}
