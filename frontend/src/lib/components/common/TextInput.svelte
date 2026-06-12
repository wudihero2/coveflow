<script lang="ts">
	// Native input attributes let callers pass type, autocomplete, required, etc.
	import type { HTMLInputAttributes } from 'svelte/elements';

	interface Props extends HTMLInputAttributes {
		label?: string;
		error?: string;
		value?: string;
		mono?: boolean;
	}

	// value is bindable so parents can use bind:value on this component.
	// Rest props are forwarded to the native input below.
	let { label, error, value = $bindable(''), mono = false, id, ...rest }: Props = $props();

	const generatedId = $props.id();
	const inputId = $derived(id ?? generatedId);
</script>

{#if label}
	<label class="block text-sm font-medium text-text-secondary" for={inputId}>{label}</label>
{/if}
<input
	id={inputId}
	class="mt-2 h-10 w-full rounded-md border border-border bg-surface px-3 text-sm text-text outline-none transition focus:border-accent focus:ring-1 focus:ring-accent {mono ? 'font-mono' : ''} {error ? 'border-error' : ''}"
	bind:value
	{...rest}
/>
{#if error}
	<p class="mt-1 text-xs text-error">{error}</p>
{/if}
