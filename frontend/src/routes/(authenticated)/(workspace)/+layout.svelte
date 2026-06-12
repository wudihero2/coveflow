<script lang="ts">
	import ResizableSplitPane from '$lib/components/common/ResizableSplitPane.svelte';
	import WorkspaceTree from '$lib/components/workspace/WorkspaceTree.svelte';

	// One explorer (scripts + flows) shared across the scripts/flows editors. The
	// tree lives in this parent layout, so it stays mounted while the right pane
	// navigates between script and flow editors (no remount / lost expand state).
	let { children } = $props();
</script>

<div class="flex h-[calc(100svh-3rem)] min-h-0 lg:h-svh">
	<ResizableSplitPane defaultPercent={22} minPrimaryPx={180} minSecondaryPx={400}>
		{#snippet primary()}
			<aside class="flex min-h-0 flex-1 flex-col border-r border-border bg-surface p-3">
				<WorkspaceTree />
			</aside>
		{/snippet}
		{#snippet secondary()}
			<div class="min-h-0 min-w-0 flex-1 overflow-auto">
				{@render children()}
			</div>
		{/snippet}
	</ResizableSplitPane>
</div>
