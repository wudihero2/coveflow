<script lang="ts">
	import {
		ChevronDown,
		ChevronRight,
		CircleCheck,
		CircleX,
		TriangleAlert
	} from '@lucide/svelte';
	import { slide } from 'svelte/transition';

	interface Props {
		success: boolean;
		result: unknown;
		expanded: boolean;
		onToggle: () => void;
	}

	let { success, result, expanded, onToggle }: Props = $props();

	const generatedId = $props.id();
	const resultBodyId = `${generatedId}-body`;

	const hasScriptError = $derived(
		success &&
			result != null &&
			typeof result === 'object' &&
			'error' in (result as Record<string, unknown>)
	);

	type Outcome = 'success' | 'script-error' | 'failure';
	const outcome = $derived<Outcome>(
		hasScriptError ? 'script-error' : success ? 'success' : 'failure'
	);

	const borderClass = $derived(
		outcome === 'success'
			? 'border-success/30'
			: outcome === 'script-error'
				? 'border-warning/30'
				: 'border-error/30'
	);

	let resultDisplay = $derived.by(() => {
		if (typeof result === 'string') return result;
		try {
			return JSON.stringify(result, null, 2);
		} catch (e) {
			return `(Unable to stringify result: ${e instanceof Error ? e.message : String(e)})`;
		}
	});
</script>

<div transition:slide={{ duration: 180 }} class="border-t {borderClass} bg-surface-raised">
	<div class="flex items-center text-xs">
		<button
			type="button"
			class="flex flex-1 items-center gap-2 px-3 py-1.5 text-left hover:bg-surface-alt"
			aria-expanded={expanded}
			aria-controls={expanded ? resultBodyId : undefined}
			onclick={onToggle}
		>
			{#if expanded}
				<ChevronDown size={12} class="text-text-tertiary" />
			{:else}
				<ChevronRight size={12} class="text-text-tertiary" />
			{/if}
			{#if outcome === 'success'}
				<CircleCheck size={14} class="text-success" />
				<span class="font-medium text-success">Success</span>
			{:else if outcome === 'script-error'}
				<TriangleAlert size={14} class="text-warning" />
				<span class="font-medium text-warning">Script Error</span>
				<span class="text-text-tertiary">— the script threw an exception but the sandbox completed normally</span>
			{:else}
				<CircleX size={14} class="text-error" />
				<span class="font-medium text-error">Failed</span>
			{/if}
			<span class="flex-1"></span>
			<span class="text-text-tertiary">{expanded ? 'Hide' : 'Show'}</span>
		</button>
	</div>
	{#if expanded}
		<pre
			id={resultBodyId}
			transition:slide={{ duration: 120 }}
			class="max-h-64 overflow-auto whitespace-pre-wrap break-words bg-surface-sunken px-3 py-2 font-mono text-xs text-text">{resultDisplay}</pre>
	{/if}
</div>
