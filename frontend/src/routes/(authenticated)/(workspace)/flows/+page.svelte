<script lang="ts">
	import { Code, Workflow } from '@lucide/svelte';
</script>

<svelte:head>
	<title>Files | CoveFlow</title>
</svelte:head>

<div class="flex h-full flex-col items-center justify-center gap-2 p-10 text-center text-text-tertiary">
	<div class="flex items-center gap-2 opacity-50">
		<Code size={30} />
		<Workflow size={30} />
	</div>
	<p class="text-sm">Select a script or flow from the tree, or create a new one.</p>
</div>
