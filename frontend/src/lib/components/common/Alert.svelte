<script lang="ts">
	// Snippet is the caller-provided alert body, similar to a typed slot.
	import type { Snippet } from 'svelte';

	interface Props {
		variant?: 'error' | 'success' | 'info';
		children: Snippet;
	}

	let { variant = 'error', children }: Props = $props();

	// Keep the variant classes centralized so callers only choose intent.
	const styles: Record<string, string> = {
		error: 'border-error/30 bg-error/5 text-error',
		success: 'border-success/30 bg-success/5 text-success',
		info: 'border-accent/30 bg-accent/5 text-accent'
	};
</script>

<div class="rounded-md border px-4 py-3 text-sm {styles[variant]}">
	{@render children()}
</div>
