<script lang="ts">
	// Snippet is the icon passed between <IconButton>...</IconButton>.
	import type { Snippet } from 'svelte';
	// Native button attributes let callers pass aria-label, title, onclick, etc.
	import type { HTMLButtonAttributes } from 'svelte/elements';

	interface Props extends HTMLButtonAttributes {
		variant?: 'ghost' | 'danger';
		size?: 'sm' | 'md';
		children: Snippet;
	}

	let {
		variant = 'ghost',
		size = 'sm',
		type = 'button',
		children,
		class: className = '',
		...rest
	}: Props = $props();

	const base = 'inline-flex items-center justify-center rounded-md transition';

	const variants: Record<string, string> = {
		ghost: 'text-text-tertiary hover:bg-surface-alt hover:text-text',
		danger: 'text-text-tertiary hover:bg-surface-alt hover:text-error'
	};

	const sizes: Record<string, string> = {
		sm: 'p-1.5',
		md: 'p-2'
	};
</script>

<!-- Compact button for icon-only actions; callers must provide accessible labels. -->
<button {type} class="{base} {variants[variant]} {sizes[size]} {className}" {...rest}>
	{@render children()}
</button>
