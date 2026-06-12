<script lang="ts">
	// Third-party icon components used for the show/hide password button.
	import { Eye, EyeOff } from '@lucide/svelte';
	// Native input attributes let callers pass autocomplete, required, minlength, etc.
	import type { HTMLInputAttributes } from 'svelte/elements';

	// This component owns the input type, so callers cannot override it.
	interface Props extends Omit<HTMLInputAttributes, 'type'> {
		label?: string;
		error?: string;
		value?: string;
	}

	// value is bindable so auth forms can use bind:value.
	let { label, error, value = $bindable(''), id, ...rest }: Props = $props();

	const generatedId = $props.id();
	const inputId = $derived(id ?? generatedId);

	// Local UI state only controls masking; it is not part of the form data.
	let visible = $state(false);
</script>

{#if label}
	<label class="block text-sm font-medium text-text-secondary" for={inputId}>{label}</label>
{/if}
<div class="relative mt-2">
	<input
		id={inputId}
		type={visible ? 'text' : 'password'}
		class="h-10 w-full rounded-md border border-border bg-surface px-3 pr-11 text-sm text-text outline-none transition focus:border-accent focus:ring-1 focus:ring-accent {error
			? 'border-error'
			: ''}"
		bind:value
		{...rest}
	/>
	<!-- Separate native button avoids submitting the form when toggling visibility. -->
	<button
		type="button"
		class="absolute inset-y-0 right-0 flex w-10 items-center justify-center rounded-r-md text-text-tertiary transition hover:text-text focus:outline-none focus:ring-1 focus:ring-accent"
		aria-label={visible ? 'Hide password' : 'Show password'}
		aria-pressed={visible}
		title={visible ? 'Hide password' : 'Show password'}
		onclick={() => {
			visible = !visible;
		}}
	>
		{#if visible}
			<EyeOff size={16} aria-hidden="true" />
		{:else}
			<Eye size={16} aria-hidden="true" />
		{/if}
	</button>
</div>
{#if error}
	<p class="mt-1 text-xs text-error">{error}</p>
{/if}
