<script lang="ts">
	// Native textarea attributes let callers pass placeholder, required, rows, etc.
	import type { HTMLTextareaAttributes } from 'svelte/elements';

	interface Props extends HTMLTextareaAttributes {
		label?: string;
		value?: string;
		mono?: boolean;
	}

	// value is bindable so forms can keep textarea state in the parent.
	// Rest props are forwarded to the native textarea below.
	let { label, value = $bindable(''), mono = false, id, ...rest }: Props = $props();

	const generatedId = $props.id();
	const inputId = $derived(id ?? generatedId);
</script>

{#if label}
	<label class="block text-sm font-medium text-text-secondary" for={inputId}>{label}</label>
{/if}
<textarea
	id={inputId}
	class="mt-2 min-h-44 w-full resize-y rounded-md border border-border bg-surface p-3 text-sm text-text outline-none transition focus:border-accent focus:ring-1 focus:ring-accent {mono ? 'font-mono' : ''}"
	bind:value
	{...rest}
></textarea>
