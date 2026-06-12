import type * as Monaco from 'monaco-editor';
import type { SnippetDef } from './types';
import { type KeywordDef, getPythonSnippets, getPythonKeywords } from './snippets';

let monacoPromise: Promise<typeof Monaco> | null = null;
let snippetsRegistered = false;

/**
 * Singleton Monaco loader. Only imports editor core + python language to
 * minimise bundle size. Workers are set up once on first call.
 */
export function getMonaco(): Promise<typeof Monaco> {
	if (monacoPromise) return monacoPromise;

	monacoPromise = (async () => {
		(globalThis as Record<string, unknown>).MonacoEnvironment = {
			getWorker() {
				return new Worker(
					new URL('monaco-editor/esm/vs/editor/editor.worker.js', import.meta.url),
					{ type: 'module' }
				);
			}
		};

		// editor.api exposes the public Monaco API; editor.all pulls in every
		// editor contribution (suggest, snippets, find, code actions, ...).
		// Without editor.all, the completion popup widget never renders even
		// though `registerCompletionItemProvider` succeeds.
		const monaco = await import('monaco-editor/esm/vs/editor/editor.api.js');
		await Promise.all([
			// @ts-expect-error — editor.all.js has no .d.ts; side-effect only import.
			import('monaco-editor/esm/vs/editor/editor.all.js'),
			import('monaco-editor/esm/vs/basic-languages/python/python.contribution.js')
		]);

		return monaco as unknown as typeof Monaco;
	})().catch((error) => {
		monacoPromise = null;
		throw error;
	});

	return monacoPromise;
}

/**
 * Register snippet + keyword completion providers once globally.
 * Safe to call from multiple ScriptEditor instances — only the first call registers.
 */
export function ensureSnippetsRegistered(monaco: typeof Monaco): void {
	if (snippetsRegistered) return;
	snippetsRegistered = true;

	registerLanguageCompletions(monaco, 'python', getPythonSnippets(), getPythonKeywords());
}

function registerLanguageCompletions(
	monaco: typeof Monaco,
	langId: string,
	snippets: SnippetDef[],
	keywords: KeywordDef[]
): void {
	monaco.languages.registerCompletionItemProvider(langId, {
		provideCompletionItems(model, position) {
			const word = model.getWordUntilPosition(position);
			const range = {
				startLineNumber: position.lineNumber,
				endLineNumber: position.lineNumber,
				startColumn: word.startColumn,
				endColumn: word.endColumn
			};
			return {
				suggestions: [
					...snippets.map((s) => ({
						label: s.label,
						kind: monaco.languages.CompletionItemKind.Snippet,
						insertText: s.insertText,
						insertTextRules:
							monaco.languages.CompletionItemInsertTextRule.InsertAsSnippet,
						detail: s.detail,
						range
					})),
					...keywords.map((k) => ({
						label: k.label,
						kind:
							k.kind === 'function'
								? monaco.languages.CompletionItemKind.Function
								: monaco.languages.CompletionItemKind.Keyword,
						insertText: k.label,
						detail: k.detail,
						range
					}))
				]
			};
		}
	});
}

/**
 * Registry for per-editor external completion providers.
 * A single global provider is registered per language; it dispatches to
 * the correct editor's onComplete by matching the model URI.
 */
type ExternalCompleteHandler = (
	model: Monaco.editor.ITextModel,
	position: Monaco.Position
) => Promise<Monaco.languages.CompletionList>;

const externalHandlers = new Map<string, ExternalCompleteHandler>();
let externalProviderRegistered = false;

function ensureExternalProvider(monaco: typeof Monaco): void {
	if (externalProviderRegistered) return;
	externalProviderRegistered = true;

	monaco.languages.registerCompletionItemProvider('python', {
		async provideCompletionItems(model, position) {
			const handler = externalHandlers.get(model.uri.toString());
			if (!handler) return { suggestions: [] };
			return handler(model, position);
		}
	});
}

/**
 * Register an external completion handler for a specific model URI.
 * Returns a disposable that removes the handler on cleanup.
 */
export function registerExternalHandler(
	monaco: typeof Monaco,
	modelUri: string,
	handler: ExternalCompleteHandler
): Monaco.IDisposable {
	ensureExternalProvider(monaco);
	externalHandlers.set(modelUri, handler);
	return {
		dispose() {
			externalHandlers.delete(modelUri);
		}
	};
}
