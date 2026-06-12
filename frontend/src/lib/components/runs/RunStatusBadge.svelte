<script lang="ts">
	import Badge from '$lib/components/common/Badge.svelte';
	import type { RunStatus } from '$lib/types';

	interface Props {
		status: RunStatus;
		/** Render with a tabular-friendly fixed width so column cells line up. */
		uniform?: boolean;
		class?: string;
	}

	let { status, uniform = false, class: className = '' }: Props = $props();

	// Pulsing the dot rather than the whole chip keeps the label legible while
	// still drawing the eye to live runs in a long list.
	const config: Record<
		RunStatus,
		{
			variant: 'success' | 'error' | 'warning' | 'info' | 'ghost';
			label: string;
			dot: string;
		}
	> = {
		queued: { variant: 'ghost', label: 'Queued', dot: 'bg-text-tertiary' },
		running: { variant: 'info', label: 'Running', dot: 'bg-info' },
		success: { variant: 'success', label: 'Success', dot: 'bg-success' },
		failure: { variant: 'error', label: 'Failed', dot: 'bg-error' },
		cancelled: { variant: 'warning', label: 'Cancelled', dot: 'bg-warning' }
	};

	const entry = $derived(config[status]);
</script>

<Badge
	variant={entry.variant}
	class="gap-1.5 {uniform ? 'w-24 justify-center' : ''} {className}"
>
	<span
		class="inline-block size-1.5 rounded-full {entry.dot} {status === 'running'
			? 'animate-pulse'
			: ''}"
		aria-hidden="true"
	></span>
	{entry.label}
</Badge>
