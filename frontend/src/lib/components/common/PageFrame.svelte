<script lang="ts">
	// Snippet lets each page provide its main body while sharing the same frame.
	import type { Snippet } from 'svelte';

	import PageHeader from './PageHeader.svelte';

	interface Props {
		title: string;
		subtitle?: string;
		actions?: Snippet;
		children: Snippet;
	}

	let { title, subtitle, actions, children }: Props = $props();
</script>

<!-- Shared page skeleton used inside the authenticated app shell. -->
<div class="flex min-h-full w-full flex-col">
	<div class="border-b border-border px-5 py-5 sm:px-8 lg:px-10">
		<PageHeader {title} {subtitle} {actions} />
	</div>
	<div class="flex-1 px-5 py-6 sm:px-8 lg:px-10">
		{@render children()}
	</div>
</div>
