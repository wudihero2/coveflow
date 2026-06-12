<script lang="ts">
	import Badge from '$lib/components/common/Badge.svelte';
	import { LABELS } from './labels';
	import type { EditorDiagnostic } from './types';

	interface Props {
		diagnostics: EditorDiagnostic[];
		onSelect: (diagnostic: EditorDiagnostic) => void;
	}

	let { diagnostics, onSelect }: Props = $props();

	const SEVERITY_MAP: Record<string, 'success' | 'error' | 'warning' | 'info' | 'ghost'> = {
		error: 'error',
		warning: 'warning',
		info: 'info'
	};

	function diagnosticKey(diag: EditorDiagnostic): string {
		return [
			diag.severity,
			diag.source ?? '',
			diag.code ?? '',
			diag.line,
			diag.column,
			diag.endLine ?? '',
			diag.endColumn ?? '',
			diag.message
		].join('|');
	}

	const keyedDiagnostics = $derived.by(() => {
		const seen: Record<string, number> = {};

		return diagnostics.map((diagnostic) => {
			const baseKey = diagnosticKey(diagnostic);
			const occurrence = seen[baseKey] ?? 0;
			seen[baseKey] = occurrence + 1;

			return {
				diagnostic,
				key: occurrence === 0 ? baseKey : `${baseKey}|${occurrence}`
			};
		});
	});
</script>

{#if diagnostics.length > 0}
	<div class="max-h-36 overflow-y-auto border-t border-border">
		<div class="px-3 py-1 text-xs font-semibold text-text-tertiary uppercase">
			{LABELS.diagnostics.title} ({diagnostics.length})
		</div>
		{#each keyedDiagnostics as item (item.key)}
			{@const diag = item.diagnostic}
			<button
				type="button"
				class="flex w-full items-start gap-2 px-3 py-1 text-left text-xs transition hover:bg-surface-alt"
				aria-label={`Go to ${diag.severity} at line ${diag.line}, column ${diag.column}`}
				onclick={() => onSelect(diag)}
			>
				<Badge variant={SEVERITY_MAP[diag.severity] ?? 'ghost'} class="shrink-0 mt-0.5">
					{diag.severity}
				</Badge>
				<span class="flex-1 text-text-secondary">
					{diag.message}
				</span>
				<span class="shrink-0 font-mono text-text-tertiary">
					{diag.line}:{diag.column}
				</span>
			</button>
		{/each}
	</div>
{/if}
