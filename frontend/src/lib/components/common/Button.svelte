<script lang="ts">
	// Third-party spinner icon for the loading state.
	import { LoaderCircle } from '@lucide/svelte';
	// Snippet is the button label/content passed between <Button>...</Button>.
	import type { Snippet } from 'svelte';
	// Native button attributes let callers pass onclick, disabled, aria-*, etc.
	import type { HTMLButtonAttributes } from 'svelte/elements';

	interface Props extends HTMLButtonAttributes {
		variant?: 'primary' | 'secondary' | 'ghost' | 'danger';
		size?: 'sm' | 'md' | 'lg';
		loading?: boolean;
		children: Snippet;
	}

	let {
		variant = 'secondary',
		size = 'md',
		loading = false,
		type = 'button',
		disabled,
		children,
		class: className = '',
		...rest
	}: Props = $props();

	const base = 'inline-flex items-center justify-center gap-2 rounded-md font-medium transition';
	let disabledState = $derived(
		loading
			? 'disabled:cursor-wait disabled:opacity-70'
			: 'disabled:cursor-not-allowed disabled:opacity-45'
	);

	const variants: Record<string, string> = {
		primary: 'bg-accent text-white hover:bg-accent-hover',
		secondary: 'border border-border text-text hover:border-border-strong hover:bg-surface-alt',
		ghost: 'text-text-secondary hover:bg-surface-alt',
		danger: 'bg-error text-white hover:bg-error/90'
	};

	const sizes: Record<string, string> = {
		sm: 'h-8 px-2.5 text-xs',
		md: 'h-10 px-3 text-sm',
		lg: 'h-11 px-4 text-sm'
	};
</script>

<!-- Wrapper around native button; rest props are forwarded to preserve ergonomics. -->
<button
	{type}
	class="{base} {variants[variant]} {sizes[size]} {disabledState} {className}"
	disabled={disabled || loading}
	{...rest}
>
	{#if loading}
		<LoaderCircle size={16} class="animate-spin" />
	{/if}
	{@render children()}
</button>
