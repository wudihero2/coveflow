<script lang="ts">
	import { untrack } from 'svelte';
	import type * as Monaco from 'monaco-editor';
	import type { ScriptLang } from '$lib/types';
	import type {
		EditorTheme,
		EditorStatus,
		EditorDiagnostic,
		ValidateScript,
		FormatScript,
		CompleteScript
	} from './editor/types';
	import { getMonacoLanguage, DEFAULT_PYTHON_RUNTIME } from './editor/languages';
	import { LABELS } from './editor/labels';
	import { getMonaco, ensureSnippetsRegistered, registerExternalHandler } from './editor/monaco';

	import Alert from '$lib/components/common/Alert.svelte';
	import EditorDiagnostics from './editor/EditorDiagnostics.svelte';
	import EditorFallback from './editor/EditorFallback.svelte';
	import EditorRequirementsPanel from './editor/EditorRequirementsPanel.svelte';
	import EditorToolbar from './editor/EditorToolbar.svelte';

	interface Props {
		content?: string;
		language?: ScriptLang;
		runtime?: string;
		requirements?: string[];
		readonly?: boolean;
		disabled?: boolean;
		theme?: EditorTheme;
		showRequirements?: boolean;
		showRuntime?: boolean;
		validateOnChange?: boolean;
		validationDebounceMs?: number;
		onRun?: (content: string) => void | Promise<void>;
		onValidate?: ValidateScript;
		onFormat?: FormatScript;
		onComplete?: CompleteScript;
	}

	let {
		content = $bindable(''),
		language = $bindable<ScriptLang>('python3'),
		runtime = $bindable(DEFAULT_PYTHON_RUNTIME),
		requirements = $bindable<string[]>([]),
		readonly: readonlyProp = false,
		disabled = false,
		theme = 'light',
		showRequirements = true,
		showRuntime = false,
		validateOnChange = false,
		validationDebounceMs = 600,
		onRun,
		onValidate,
		onFormat,
		onComplete
	}: Props = $props();

	let editorContainer: HTMLDivElement | undefined = $state();
	let editor: Monaco.editor.IStandaloneCodeEditor | undefined = $state();
	let monacoRef: typeof Monaco | undefined = $state();
	let monacoFailed = $state(false);
	let status: EditorStatus = $state('loading');
	let diagnostics: EditorDiagnostic[] = $state([]);
	let actionError: string | null = $state(null);
	let requirementsText = $state('');
	let requirementsOpen = $state(false);
	let disposables: Monaco.IDisposable[] = [];
	let validateTimer: ReturnType<typeof setTimeout> | undefined;
	let validationSeq = 0;
	let updatingFromProp = false;
	let updatingFromEditor = false;
	let lastRequirementsValue = $state('');

	function resolveTheme(t: EditorTheme): string {
		if (t === 'dark') return 'vs-dark';
		if (t === 'light') return 'vs';
		if (typeof window !== 'undefined' && window.matchMedia('(prefers-color-scheme: dark)').matches) {
			return 'vs-dark';
		}
		return 'vs';
	}

	function getMonacoSeverity(
		monaco: typeof Monaco,
		severity: string
	): Monaco.MarkerSeverity {
		switch (severity) {
			case 'error':
				return monaco.MarkerSeverity.Error;
			case 'warning':
				return monaco.MarkerSeverity.Warning;
			default:
				return monaco.MarkerSeverity.Info;
		}
	}

	function errorMessage(e: unknown): string {
		return e instanceof Error ? e.message : 'An unexpected error occurred';
	}

	// Initialize Monaco editor
	$effect(() => {
		if (!editorContainer) return;

		let disposed = false;
		const container = editorContainer;

		(async () => {
			try {
				const monaco = await getMonaco();
				if (disposed) return;

				monacoRef = monaco;
				const langId = getMonacoLanguage(language);
				const model = monaco.editor.createModel(content, langId);

				const instance = monaco.editor.create(container, {
					model,
					automaticLayout: true,
					minimap: { enabled: false },
					fontSize: 14,
					fontFamily: "'JetBrains Mono', 'Fira Code', 'Cascadia Code', monospace",
					readOnly: readonlyProp || disabled,
					lineNumbers: 'on',
					wordBasedSuggestions: 'currentDocument',
					scrollBeyondLastLine: false,
					theme: resolveTheme(theme),
					padding: { top: 8, bottom: 8 },
					renderLineHighlight: 'line',
					overviewRulerLanes: 0,
					hideCursorInOverviewRuler: true,
					overviewRulerBorder: false,
					scrollbar: {
						verticalScrollbarSize: 8,
						horizontalScrollbarSize: 8
					}
				});

				if (disposed) {
					instance.dispose();
					model.dispose();
					return;
				}

				editor = instance;

				// Sync editor changes to content prop
				const changeDisposable = instance.onDidChangeModelContent(() => {
					if (updatingFromProp) return;
					updatingFromEditor = true;
					content = instance.getValue();
					updatingFromEditor = false;

					actionError = null;

					if (validateOnChange && onValidate && !disabled) {
						clearTimeout(validateTimer);
						validateTimer = setTimeout(() => {
							void runValidation();
						}, validationDebounceMs);
					}
				});
				disposables.push(changeDisposable);

				ensureSnippetsRegistered(monaco);

				status = 'idle';
			} catch {
				if (!disposed) {
					monacoFailed = true;
					status = 'idle';
				}
			}
		})();

		return () => {
			disposed = true;
			clearTimeout(validateTimer);
			for (const d of disposables) d.dispose();
			disposables = [];
			if (editor) {
				const model = editor.getModel();
				editor.dispose();
				model?.dispose();
				editor = undefined;
			}
			monacoRef = undefined;
		};
	});

	// Register optional external completions for the current model only.
	$effect(() => {
		if (!editor || !monacoRef || !onComplete) return;

		const model = editor.getModel();
		if (!model) return;
		const monaco = monacoRef;

		const handler = registerExternalHandler(monaco, model.uri.toString(), async (m, position) => {
			const word = m.getWordUntilPosition(position);
			const range = {
				startLineNumber: position.lineNumber,
				endLineNumber: position.lineNumber,
				startColumn: word.startColumn,
				endColumn: word.endColumn
			};
			const items = await onComplete({
				content: m.getValue(),
				language,
				line: position.lineNumber,
				column: position.column
			});
			return {
				suggestions: items.map((item) => ({
					label: item.label,
					kind: monaco.languages.CompletionItemKind.Snippet,
					insertText: item.insertText,
					detail: item.detail,
					documentation: item.documentation,
					sortText: item.sortText,
					range
				}))
			};
		});

		return () => handler.dispose();
	});

	// Sync language changes to Monaco model
	$effect(() => {
		if (!editor || !monacoRef) return;
		const model = editor.getModel();
		if (!model) return;
		const langId = getMonacoLanguage(language);
		monacoRef.editor.setModelLanguage(model, langId);
	});

	// Sync external content changes to Monaco
	$effect(() => {
		const currentContent = content;
		if (!editor || updatingFromEditor) return;
		if (currentContent !== editor.getValue()) {
			updatingFromProp = true;
			editor.setValue(currentContent);
			updatingFromProp = false;
		}
	});

	// Sync readonly/disabled to Monaco
	$effect(() => {
		if (!editor) return;
		editor.updateOptions({ readOnly: readonlyProp || disabled });
	});

	// Sync theme to Monaco
	$effect(() => {
		if (!monacoRef) return;
		monacoRef.editor.setTheme(resolveTheme(theme));
	});

	// Sync requirements prop to text without normalizing in-progress user input.
	$effect(() => {
		const text = requirements.join(', ');
		if (text === untrack(() => lastRequirementsValue)) return;
		const current = untrack(() => requirementsText);
		if (text !== current) {
			requirementsText = text;
		}
		lastRequirementsValue = text;
	});

	function parseRequirements(value: string): string[] {
		return value
			.split(',')
			.map((s) => s.trim())
			.filter(Boolean);
	}

	function onRequirementsInput(value: string) {
		requirementsText = value;
		requirements = parseRequirements(value);
		lastRequirementsValue = requirements.join(', ');
	}

	async function runValidation() {
		if (!onValidate || disabled) return;
		actionError = null;
		status = 'validating';
		const seq = ++validationSeq;
		try {
			const result = await onValidate(content, language);
			if (seq !== validationSeq) return;

			diagnostics = result;

			// Set Monaco markers if editor is available (skipped in fallback mode)
			const model = editor?.getModel();
			if (model && monacoRef) {
				const markers = result.map((d) => ({
					severity: getMonacoSeverity(monacoRef!, d.severity),
					message: d.message,
					startLineNumber: d.line,
					startColumn: d.column,
					endLineNumber: d.endLine ?? d.line,
					endColumn: d.endColumn ?? d.column + 1,
					source: d.source
				}));
				monacoRef.editor.setModelMarkers(model, 'script-editor', markers);
			}
		} catch (e) {
			if (seq !== validationSeq) return;
			actionError = `Validation failed: ${errorMessage(e)}`;
		} finally {
			if (seq === validationSeq) {
				status = 'idle';
			}
		}
	}

	async function runFormat() {
		if (!onFormat || disabled || readonlyProp) return;
		actionError = null;
		status = 'formatting';
		try {
			const formatted = await onFormat(content, language);
			content = formatted;
		} catch (e) {
			actionError = `Format failed: ${errorMessage(e)}`;
		} finally {
			status = 'idle';
		}
	}

	async function runScript() {
		if (!onRun || disabled || readonlyProp) return;
		actionError = null;
		status = 'running';
		try {
			await onRun(content);
		} catch (e) {
			actionError = `Run failed: ${errorMessage(e)}`;
		} finally {
			status = 'idle';
		}
	}

	function goToDiagnostic(d: EditorDiagnostic) {
		if (!editor) return;
		editor.revealLineInCenter(d.line);
		editor.setPosition({ lineNumber: d.line, column: d.column });
		editor.focus();
	}

	const showReqInput = $derived(showRequirements && language === 'python3');
	const showRuntimeSelect = $derived(showRuntime && language === 'python3');

	$effect(() => {
		if (!showReqInput) {
			requirementsOpen = false;
		}
	});
</script>

<div
	class="flex min-h-0 flex-1 flex-col overflow-hidden border border-border"
	class:opacity-60={disabled}
>
	<EditorToolbar
		bind:language
		bind:runtime
		bind:requirementsOpen
		showRequirements={showReqInput}
		showRuntime={showRuntimeSelect}
		requirementsCount={requirements.length}
		{disabled}
		readonly={readonlyProp}
		{status}
		{diagnostics}
		onValidate={onValidate ? runValidation : undefined}
		onFormat={onFormat ? runFormat : undefined}
		onRun={onRun ? runScript : undefined}
	/>

	{#if actionError}
		<div class="border-b border-border">
			<Alert variant="error">{actionError}</Alert>
		</div>
	{/if}

	{#if showReqInput && requirementsOpen}
		<EditorRequirementsPanel
			bind:value={requirementsText}
			{disabled}
			readonly={readonlyProp}
			onValueChange={onRequirementsInput}
		/>
	{/if}

	{#if monacoFailed}
		<EditorFallback bind:content {disabled} readonly={readonlyProp} />
	{:else}
		<div class="relative min-h-0 flex-1">
			<div bind:this={editorContainer} class="absolute inset-0"></div>
			{#if status === 'loading'}
				<div class="absolute inset-0 flex items-center justify-center text-sm text-text-tertiary">
					{LABELS.status.loading}
				</div>
			{/if}
		</div>
	{/if}

	<EditorDiagnostics {diagnostics} onSelect={goToDiagnostic} />
</div>
