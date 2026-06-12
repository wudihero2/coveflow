import { browser } from '$app/environment';

import type { ScriptLang } from '$lib/types';

const DRAFT_PREFIX = 'coveflow:script-draft:v1';

export interface ScriptDraft {
	version: 1;
	workspaceId: string;
	scriptPath: string;
	baseHash: string;
	content: string;
	language: ScriptLang;
	runtime: string;
	requirements: string[];
	summary: string;
	updatedAt: string;
}

export interface ScriptDraftBaseline {
	content: string;
	language: ScriptLang;
	runtime: string;
	requirements: string[];
}

function keyFor(workspaceId: string, scriptPath: string): string {
	return `${DRAFT_PREFIX}:${encodeURIComponent(workspaceId)}:${encodeURIComponent(scriptPath)}`;
}

function isScriptDraft(value: unknown): value is ScriptDraft {
	if (!value || typeof value !== 'object') return false;
	const draft = value as Record<string, unknown>;
	return (
		draft.version === 1 &&
		typeof draft.workspaceId === 'string' &&
		typeof draft.scriptPath === 'string' &&
		typeof draft.baseHash === 'string' &&
		typeof draft.content === 'string' &&
		draft.language === 'python3' &&
		typeof draft.runtime === 'string' &&
		Array.isArray(draft.requirements) &&
		draft.requirements.every((item) => typeof item === 'string') &&
		typeof draft.summary === 'string' &&
		typeof draft.updatedAt === 'string'
	);
}

export function readScriptDraft(workspaceId: string, scriptPath: string): ScriptDraft | null {
	if (!browser) return null;

	try {
		const raw = sessionStorage.getItem(keyFor(workspaceId, scriptPath));
		if (!raw) return null;
		const parsed = JSON.parse(raw);
		if (!isScriptDraft(parsed)) return null;
		if (parsed.workspaceId !== workspaceId || parsed.scriptPath !== scriptPath) return null;
		return parsed;
	} catch {
		return null;
	}
}

export function writeScriptDraft(draft: ScriptDraft): boolean {
	if (!browser) return false;

	try {
		sessionStorage.setItem(keyFor(draft.workspaceId, draft.scriptPath), JSON.stringify(draft));
		return true;
	} catch {
		return false;
	}
}

export function removeScriptDraft(workspaceId: string, scriptPath: string): void {
	if (!browser) return;

	try {
		sessionStorage.removeItem(keyFor(workspaceId, scriptPath));
	} catch {
		// Storage can be unavailable in private or restricted browser contexts.
	}
}

export function draftDiffersFromBaseline(
	draft: ScriptDraft,
	baseline: ScriptDraftBaseline
): boolean {
	return (
		draft.content !== baseline.content ||
		draft.language !== baseline.language ||
		draft.runtime !== baseline.runtime ||
		draft.requirements.join('\n') !== baseline.requirements.join('\n')
	);
}
