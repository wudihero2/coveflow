<script lang="ts">
	// Compact inline usage bar: a filled track plus a "used / total unit" label.
	// Fill color escalates with utilization so a saturated worker stands out.
	interface Props {
		used: number;
		total: number;
		unit?: string;
		decimals?: number;
	}

	let { used, total, unit = '', decimals = 1 }: Props = $props();

	const pct = $derived(total > 0 ? Math.max(0, Math.min(100, (used / total) * 100)) : 0);
	const fill = $derived(pct >= 90 ? 'bg-error' : pct >= 75 ? 'bg-warning' : 'bg-accent');
	const fmt = (v: number): string => v.toFixed(decimals);
</script>

<div class="flex items-center gap-2">
	<div class="h-1.5 w-16 flex-shrink-0 overflow-hidden rounded-full bg-surface-sunken">
		<div class="h-full rounded-full {fill} transition-[width]" style:width="{pct}%"></div>
	</div>
	<span class="text-xs whitespace-nowrap text-text-secondary tabular-nums">
		{fmt(used)} / {fmt(total)}{unit ? ` ${unit}` : ''}
	</span>
</div>
