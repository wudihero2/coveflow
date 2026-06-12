<script lang="ts">
	import type { ClusterSummary } from '$lib/types';

	interface Props {
		summary: ClusterSummary | null;
	}

	let { summary }: Props = $props();

	const utilizationPct = $derived(summary ? Math.round(summary.utilization * 100) : 0);
	// Workers registered but not pinging: capacity/utilization above counts only
	// the alive ones, so surface the gap to explain why e.g. "5 workers · 100%"
	// can coexist with idle-looking hardware.
	const staleCount = $derived(summary ? summary.workers_total - summary.workers_alive : 0);
</script>

{#if summary}
	<div
		class="mb-4 flex flex-wrap items-center gap-x-8 gap-y-1 rounded-lg border border-border bg-surface-raised px-4 py-3 text-sm"
	>
		<span>
			<span class="font-semibold text-text tabular-nums">{summary.total_cpus.toFixed(1)}</span>
			<span class="text-text-tertiary">total cpu</span>
		</span>
		<span>
			<span class="font-semibold text-text tabular-nums">{summary.used_cpus.toFixed(1)}</span>
			<span class="text-text-tertiary">used</span>
		</span>
		<span>
			<span class="font-semibold text-text tabular-nums">{utilizationPct}%</span>
			<span class="text-text-tertiary">utilization</span>
		</span>
		<span>
			<span class="font-semibold text-text tabular-nums"
				>{summary.workers_alive}/{summary.workers_total}</span
			>
			<span class="text-text-tertiary">workers up</span>
			{#if staleCount > 0}
				<span class="text-warning">· {staleCount} stale</span>
			{/if}
		</span>
	</div>
{/if}
