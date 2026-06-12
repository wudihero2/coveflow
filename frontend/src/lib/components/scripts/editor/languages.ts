import type { ScriptLang } from '$lib/types';

const LANGUAGE_MAP: Record<ScriptLang, string> = {
	python3: 'python'
};

export const LANGUAGE_OPTIONS: { label: string; value: ScriptLang }[] = [
	{ label: 'Python 3', value: 'python3' }
];

/**
 * Supported Python runtime versions.
 * Maps to backend worker config (coveflow/crates/worker/src/config.rs).
 * Default is python:3.12.
 */
export const PYTHON_RUNTIME_OPTIONS: { label: string; value: string }[] = [
	{ label: '3.12', value: 'python:3.12' },
	{ label: '3.11', value: 'python:3.11' }
];

export const DEFAULT_PYTHON_RUNTIME = 'python:3.12';

export function getMonacoLanguage(lang: ScriptLang): string {
	return LANGUAGE_MAP[lang] ?? 'plaintext';
}
