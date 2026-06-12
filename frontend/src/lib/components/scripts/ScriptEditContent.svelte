<script lang="ts">
	import type { Snippet } from 'svelte';
	import Alert from '$lib/components/common/Alert.svelte';
	import ResizableSplitPane from '$lib/components/common/ResizableSplitPane.svelte';

	interface Props {
		loading: boolean;
		error: string;
		loaded: boolean;
		children: Snippet;
		/** Right-side panel such as run logs and result details. */
		right?: Snippet;
	}

	let { loading, error, loaded, children, right }: Props = $props();
</script>

{#if loading}
	<div class="flex flex-1 items-center justify-center text-text-tertiary">Loading...</div>
{:else if error}
	<div class="p-4">
		<Alert variant="error">{error}</Alert>
	</div>
{:else if !loaded}
	<div class="flex flex-1 items-center justify-center text-text-tertiary">Script is not loaded.</div>
{:else if right}
	<ResizableSplitPane
		primary={children}
		secondary={right}
		defaultPercent={60}
		storageKey="script-edit-output-split"
		ariaLabel="Resize editor and run output panels"
	/>
{:else}
	{@render children()}
{/if}
