<script lang="ts">
	import type { Snippet } from 'svelte';

	interface Props {
		label: string;
		children: Snippet;
	}

	let { label, children }: Props = $props();
</script>

<div>
	<dt class="font-medium uppercase tracking-wider text-text-tertiary">{label}</dt>
	<dd class="mt-0.5">{@render children()}</dd>
</div>
