<script lang="ts">
	import type { HTMLInputAttributes } from 'svelte/elements';
	import type { Snippet } from 'svelte';

	interface Props extends Omit<HTMLInputAttributes, 'type' | 'checked'> {
		label?: string;
		description?: Snippet | string;
		checked?: boolean;
	}

	let {
		label,
		description,
		checked = $bindable(false),
		id,
		disabled = false,
		class: className = '',
		...rest
	}: Props = $props();

	const generatedId = $props.id();
	const inputId = $derived(id ?? generatedId);
</script>

<label
	for={inputId}
	class="flex cursor-pointer items-start gap-2 {disabled ? 'cursor-not-allowed opacity-60' : ''} {className}"
>
	<input
		{...rest}
		id={inputId}
		type="checkbox"
		bind:checked
		{disabled}
		class="mt-0.5 size-4 shrink-0 rounded border-border accent-accent focus:ring-1 focus:ring-accent"
	/>
	<span class="min-w-0 flex-1">
		{#if label}
			<span class="block text-sm text-text">{label}</span>
		{/if}
		{#if description}
			<span class="mt-0.5 block text-xs text-text-tertiary">
				{#if typeof description === 'string'}{description}{:else}{@render description()}{/if}
			</span>
		{/if}
	</span>
</label>
