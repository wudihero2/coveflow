import type { LogEntry } from '$lib/types';

export type LogLevelLabel = 'TRACE' | 'DEBUG' | 'INFO' | 'WARN' | 'ERROR';

export const LEVEL_LABELS: Record<number, LogLevelLabel> = {
	1: 'TRACE',
	2: 'DEBUG',
	3: 'INFO',
	4: 'WARN',
	5: 'ERROR'
};

export function levelLabel(level: number): LogLevelLabel {
	return LEVEL_LABELS[level] ?? 'INFO';
}

export function levelClass(level: number): string {
	switch (level) {
		case 1:
		case 2:
			return 'text-text-tertiary';
		case 4:
			return 'text-warning';
		case 5:
			return 'text-error';
		default:
			return 'text-text';
	}
}

/**
 * Tailwind border-color class keyed by log level. The bar is meant to be
 * a peripheral severity cue, not the main signal — INFO (the common case)
 * is intentionally near-invisible so WARN/ERROR can pop. TRACE/DEBUG go
 * fully transparent.
 */
export function levelBorderClass(level: number): string {
	switch (level) {
		case 1:
		case 2:
			return 'border-transparent';
		case 4:
			return 'border-warning';
		case 5:
			return 'border-error';
		default:
			return 'border-border';
	}
}

/**
 * Render a log line timestamp as UTC ISO (`2026-06-10T06:36:00Z`). Used only for
 * copy/export, where UTC is the least ambiguous format for pasted text. On-screen
 * log timestamps go through `formatAbsolute` (display timezone + numeric offset).
 */
export function formatTimestamp(ts: string): string {
	if (!ts) return '';
	const date = new Date(ts);
	if (Number.isNaN(date.getTime())) return ts;
	const pad = (n: number) => n.toString().padStart(2, '0');
	const yyyy = date.getUTCFullYear();
	const mm = pad(date.getUTCMonth() + 1);
	const dd = pad(date.getUTCDate());
	const hh = pad(date.getUTCHours());
	const mi = pad(date.getUTCMinutes());
	const ss = pad(date.getUTCSeconds());
	return `${yyyy}-${mm}-${dd}T${hh}:${mi}:${ss}Z`;
}

export interface FormattedField {
	key: string;
	/** Rendered lines. Single-entry array for scalars/JSON; multi-entry for multi-line strings. */
	lines: string[];
}

/**
 * Format one field value into one or more display lines.
 * - string: split on newlines; preserves internal whitespace.
 * - object/array: JSON.stringify on one line.
 * - other scalars: String(value).
 */
export function formatFieldValue(value: unknown): string[] {
	if (typeof value === 'string') {
		return value.split('\n');
	}
	if (value === null || value === undefined) {
		return [String(value)];
	}
	if (typeof value === 'object') {
		try {
			return [JSON.stringify(value)];
		} catch {
			return [String(value)];
		}
	}
	return [String(value)];
}

export function formatEntryFields(
	fields: Record<string, unknown> | undefined
): FormattedField[] {
	if (!fields) return [];
	return Object.entries(fields).map(([key, value]) => ({
		key,
		lines: formatFieldValue(value)
	}));
}

export function entryToPlainText(entry: LogEntry): string {
	const ts = formatTimestamp(entry.ts);
	const lvl = levelLabel(entry.level).padEnd(5, ' ');
	const target = entry.target ? ` [${entry.target}]` : '';
	const node = entry.node_id ? ` [${entry.node_id}]` : '';
	const header = `${ts}  ${lvl}${target}${node}  ${entry.msg}`;

	const fields = formatEntryFields(entry.fields);
	if (fields.length === 0) return header;

	const indent = '    ';
	const fieldLines: string[] = [];
	for (const f of fields) {
		const isOutput = f.key === 'stdout' || f.key === 'stderr';
		if (isOutput) {
			// User-visible script output: render content directly, no key label.
			for (const line of f.lines) {
				fieldLines.push(`${indent}${line}`);
			}
		} else if (f.lines.length === 1) {
			fieldLines.push(`${indent}${f.key}: ${f.lines[0]}`);
		} else {
			fieldLines.push(`${indent}${f.key}:`);
			for (const line of f.lines) {
				fieldLines.push(`${indent}  ${line}`);
			}
		}
	}
	return `${header}\n${fieldLines.join('\n')}`;
}

export function entriesToPlainText(entries: readonly LogEntry[]): string {
	return entries.map(entryToPlainText).join('\n');
}
