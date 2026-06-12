import type { ScriptLang } from '$lib/types';

export type EditorTheme = 'light' | 'dark' | 'auto';
export type DiagnosticSeverity = 'error' | 'warning' | 'info';
export type EditorStatus = 'idle' | 'loading' | 'validating' | 'formatting' | 'running';

export interface EditorDiagnostic {
	severity: DiagnosticSeverity;
	message: string;
	line: number;
	column: number;
	endLine?: number;
	endColumn?: number;
	source?: string;
	code?: string;
}

export interface EditorCompletionContext {
	content: string;
	language: ScriptLang;
	line: number;
	column: number;
	triggerCharacter?: string;
	context?: unknown;
}

export interface EditorCompletionItem {
	label: string;
	insertText: string;
	detail?: string;
	documentation?: string;
	sortText?: string;
}

export type ValidateScript = (
	content: string,
	language: ScriptLang
) => Promise<EditorDiagnostic[]>;

export type FormatScript = (content: string, language: ScriptLang) => Promise<string>;

export type CompleteScript = (
	context: EditorCompletionContext
) => Promise<EditorCompletionItem[]>;

export interface SnippetDef {
	label: string;
	insertText: string;
	detail: string;
}
