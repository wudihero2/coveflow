<script lang="ts">
	import type { Snippet } from 'svelte';

	interface Props {
		variant?: 'success' | 'error' | 'warning' | 'info' | 'ghost';
		class?: string;
		children: Snippet;
	}

	let { variant = 'ghost', class: className = '', children }: Props = $props();

	const variants: Record<string, string> = {
		success: 'bg-emerald-100 text-emerald-800',
		error: 'bg-red-100 text-red-800',
		warning: 'bg-amber-100 text-amber-800',
		info: 'bg-blue-100 text-blue-800',
		ghost: 'bg-surface-alt text-text-tertiary'
	};
</script>

<span class="inline-flex items-center rounded-full px-2 py-0.5 text-xs font-medium {variants[variant]} {className}">
	{@render children()}
</span>
