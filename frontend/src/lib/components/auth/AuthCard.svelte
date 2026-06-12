<script lang="ts">
	// Snippet lets LoginForm/SignupForm provide the form body.
	import type { Snippet } from 'svelte';

	import CoveFlowLogo from '$lib/components/common/CoveFlowLogo.svelte';

	interface Props {
		title: string;
		children: Snippet;
	}

	let { title, children }: Props = $props();
</script>

<!-- Shared auth-page frame: brand header plus caller-provided form content. -->
<div class="w-full max-w-sm rounded-lg border border-border bg-surface-raised p-6 shadow-sm">
	<div class="mb-7 flex items-center gap-3">
		<CoveFlowLogo height={36} />
		<div>
			<p class="text-xs font-semibold text-text uppercase">CoveFlow</p>
			<h1 class="text-xl font-semibold text-text">{title}</h1>
		</div>
	</div>

	{@render children()}
</div>
