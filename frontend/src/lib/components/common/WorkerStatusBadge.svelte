<script lang="ts">
	import Badge from './Badge.svelte';

	// alive = pinged recently; stale = missed its heartbeats.
	interface Props {
		status: 'alive' | 'stale';
	}

	let { status }: Props = $props();

	const alive = $derived(status === 'alive');
</script>

<Badge variant={alive ? 'success' : 'ghost'}>
	<span
		class="mr-1 inline-block h-1.5 w-1.5 rounded-full {alive ? 'bg-success' : 'bg-text-tertiary'}"
	></span>
	{status}
</Badge>
