<script lang="ts">
	import { ArrowRight, X } from '@lucide/svelte';
	import type * as Monaco from 'monaco-editor';
	import Button from '$lib/components/common/Button.svelte';
	import IconButton from '$lib/components/common/IconButton.svelte';
	import { getMonaco } from './editor/monaco';
	import { getMonacoLanguage } from './editor/languages';
	import type { ScriptLang, ScriptResponse } from '$lib/types';
	import { formatRelative } from '$lib/utils/format-time';

	interface Props {
		open: boolean;
		/** Older version to show on the left side. */
		oldScript: ScriptResponse | null;
		/** Live editor buffer to show on the right side. */
		currentContent: string;
		currentLanguage: ScriptLang;
		onClose: () => void;
		/** Optional one-click Restore from inside the modal — collapses the
		 * "browse → close → restore" trail into a single flow. The parent owns
		 * the dirty-guard via its existing RestoreConfirmModal. */
		onRestore?: (script: ScriptResponse) => void;
	}

	let { open, oldScript, currentContent, currentLanguage, onClose, onRestore }: Props = $props();

	let dialog: HTMLDialogElement | undefined = $state();
	let container: HTMLDivElement | undefined = $state();
	let diffEditor: Monaco.editor.IStandaloneDiffEditor | undefined;
	let originalModel: Monaco.editor.ITextModel | undefined;
	let modifiedModel: Monaco.editor.ITextModel | undefined;

	// Show / hide the native <dialog> in sync with the `open` prop.
	$effect(() => {
		if (!dialog) return;
		if (open && !dialog.open) dialog.showModal();
		else if (!open && dialog.open) dialog.close();
	});

	// Mount / tear down the Monaco diff editor whenever the modal opens.
	// Done in a separate effect so we re-mount with fresh content if the user
	// closes and re-opens with a different version selected.
	$effect(() => {
		if (!open || !container || !oldScript) return;

		const oldLang = (oldScript.language as ScriptLang | null) ?? currentLanguage;
		const captured = container;
		let disposed = false;

		(async () => {
			const monaco = await getMonaco();
			if (disposed) return;

			originalModel = monaco.editor.createModel(
				oldScript.content ?? '',
				getMonacoLanguage(oldLang)
			);
			modifiedModel = monaco.editor.createModel(
				currentContent,
				getMonacoLanguage(currentLanguage)
			);

			diffEditor = monaco.editor.createDiffEditor(captured, {
				readOnly: true,
				originalEditable: false,
				renderSideBySide: true,
				// Monaco silently switches to inline diff when the editor is narrower
				// than ~900px. Force side-by-side regardless so a narrow modal still
				// shows the layout the user opted into.
				renderSideBySideInlineBreakpoint: 0,
				useInlineViewWhenSpaceIsLimited: false,
				automaticLayout: true,
				minimap: { enabled: false },
				scrollBeyondLastLine: false,
				fontSize: 13
			});
			diffEditor.setModel({ original: originalModel, modified: modifiedModel });
		})();

		return () => {
			disposed = true;
			diffEditor?.dispose();
			originalModel?.dispose();
			modifiedModel?.dispose();
			diffEditor = undefined;
			originalModel = undefined;
			modifiedModel = undefined;
		};
	});

	function onBackdropClick(e: MouseEvent) {
		if (e.target === dialog) onClose();
	}
</script>

<dialog
	bind:this={dialog}
	class="m-auto h-[85vh] w-[90vw] max-w-7xl rounded-md border border-border bg-surface-raised p-0 shadow-2xl backdrop:bg-black/40"
	onclick={onBackdropClick}
	onclose={onClose}
>
	<div class="flex h-full flex-col">
		<header class="flex items-center justify-between border-b border-border px-4 py-2">
			<!--
				Direction-aware header: "<old hash · time ago>  →  current".
				Reading the arrow left-to-right tells the user instantly what
				they're comparing against, which way is "newer", and how old
				the candidate is — no need to think in terms of "left/right".
			-->
			<div class="flex items-center gap-3 text-sm">
				{#if oldScript}
					<div class="flex items-center gap-2">
						<code class="rounded bg-surface-alt px-1.5 py-0.5 font-mono text-xs text-text">
							{oldScript.hash.slice(0, 8)}
						</code>
						<span class="text-xs text-text-tertiary">{formatRelative(oldScript.created_at)}</span>
					</div>
				{/if}
				<ArrowRight size={14} class="text-text-tertiary" />
				<span class="rounded bg-accent/15 px-1.5 py-0.5 text-xs font-medium text-accent">
					current
				</span>
			</div>
			<IconButton aria-label="Close diff" title="Close" onclick={onClose}>
				<X size={16} />
			</IconButton>
		</header>

		<div bind:this={container} class="min-h-0 flex-1"></div>

		<footer class="flex items-center justify-between border-t border-border px-4 py-2">
			<span class="text-xs text-text-tertiary">Reviewing previous version — readonly</span>
			<div class="flex gap-2">
				<Button variant="secondary" size="sm" onclick={onClose}>Close</Button>
				{#if onRestore && oldScript}
					<Button
						variant="primary"
						size="sm"
						onclick={() => {
							if (oldScript && onRestore) onRestore(oldScript);
						}}
					>
						Restore this version
					</Button>
				{/if}
			</div>
		</footer>
	</div>
</dialog>
