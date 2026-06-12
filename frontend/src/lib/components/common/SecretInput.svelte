<script lang="ts">
	import { Eye, EyeOff } from '@lucide/svelte';

	interface Props {
		/** The secret value (two-way bound). */
		value?: string;
		label?: string;
		placeholder?: string;
		/** Helper text under the field; defaults to the write-only reminder. */
		hint?: string;
	}

	let {
		value = $bindable(''),
		label,
		placeholder = 'Secret value',
		hint = 'Write-only — stored encrypted and never shown again.'
	}: Props = $props();

	const generatedId = $props.id();
	let revealed = $state(false);
</script>

{#if label}
	<label class="block text-sm font-medium text-text-secondary" for={generatedId}>{label}</label>
{/if}
<div class="relative mt-2">
	<input
		id={generatedId}
		type={revealed ? 'text' : 'password'}
		class="h-10 w-full rounded-md border border-border bg-surface px-3 pr-10 font-mono text-sm text-text outline-none transition focus:border-accent focus:ring-1 focus:ring-accent"
		bind:value
		{placeholder}
		autocomplete="off"
		autocapitalize="off"
		spellcheck="false"
	/>
	<button
		type="button"
		class="absolute inset-y-0 right-0 flex items-center px-3 text-text-tertiary transition hover:text-text"
		aria-label={revealed ? 'Hide value' : 'Show value'}
		onclick={() => (revealed = !revealed)}
	>
		{#if revealed}
			<EyeOff size={16} />
		{:else}
			<Eye size={16} />
		{/if}
	</button>
</div>
{#if hint}
	<p class="mt-1 text-xs text-text-tertiary">{hint}</p>
{/if}
