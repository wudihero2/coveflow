<script lang="ts">
	import type { Snippet } from 'svelte';

	interface Props {
		title?: string;
		children: Snippet;
	}

	let { title, children }: Props = $props();
</script>

<div class="rounded-lg border border-border bg-surface-raised p-5">
	{#if title}
		<h3 class="mb-3 text-sm font-semibold text-text">{title}</h3>
	{/if}
	{@render children()}
</div>
