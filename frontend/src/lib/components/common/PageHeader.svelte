<script lang="ts">
	// Snippet is used for optional right-side actions like Refresh buttons.
	import type { Snippet } from 'svelte';

	interface Props {
		title: string;
		subtitle?: string;
		actions?: Snippet;
	}

	let { title, subtitle, actions }: Props = $props();
</script>

<header
	class="flex flex-col gap-4 sm:flex-row sm:items-center sm:justify-between"
>
	<div>
		{#if subtitle}
			<p class="text-xs font-semibold text-text-tertiary uppercase">{subtitle}</p>
		{/if}
		<h1 class="text-xl font-semibold text-text">{title}</h1>
	</div>

	{#if actions}
		<!-- Parent pages provide actions; this component only positions them. -->
		<div class="flex gap-2">
			{@render actions()}
		</div>
	{/if}
</header>
