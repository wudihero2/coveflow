<script lang="ts">
	import type { HTMLSelectAttributes } from 'svelte/elements';

	// Simple option shape used by all dropdowns in the app.
	interface Option {
		label: string;
		value: string;
	}

	interface Props extends Omit<HTMLSelectAttributes, 'onchange'> {
		label?: string;
		// Component prop uses camelCase; it becomes aria-label on the native select.
		ariaLabel?: string;
		options: Option[];
		value?: string;
		compact?: boolean;
		onchange?: (value: string) => void;
	}

	// value is bindable, and onchange gives parents a simple selected string.
	let {
		label,
		ariaLabel,
		options,
		value = $bindable(''),
		id,
		compact = false,
		class: className = '',
		onchange,
		...rest
	}: Props = $props();

	const generatedId = $props.id();
	const selectId = $derived(id ?? generatedId);
</script>

{#if label}
	<label class="block text-sm font-medium text-text-secondary" for={selectId}>{label}</label>
{/if}
<select
	id={selectId}
	aria-label={ariaLabel}
	class="{label ? 'mt-2' : ''} w-full rounded-md border border-border bg-surface px-3 text-text outline-none transition focus:border-accent focus:ring-1 focus:ring-accent
		{compact ? 'h-9 text-xs' : 'h-10 text-sm'} {className}"
	bind:value
	onchange={(e) => onchange?.(e.currentTarget.value)}
	{...rest}
>
	<!-- Key by value so option identity stays stable when labels change. -->
	{#each options as opt (opt.value)}
		<option value={opt.value}>{opt.label}</option>
	{/each}
</select>
