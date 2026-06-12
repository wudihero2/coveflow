<script lang="ts">
	import { Copy } from '@lucide/svelte';
	import Badge from '$lib/components/common/Badge.svelte';
	import DataTable from '$lib/components/common/DataTable.svelte';
	import RunStatusBadge from './RunStatusBadge.svelte';
	import type { RunListItem } from '$lib/types';
	import { formatDuration } from '$lib/utils/format-duration';
	import { formatRelative, formatAbsolute } from '$lib/utils/format-time';
	import { displayTz } from '$lib/stores/timezone.svelte';

	type RunColumnKey = Extract<keyof RunListItem, string>;

	interface Props {
		rows: readonly RunListItem[];
		loading?: boolean;
		emptyText?: string;
		onRowClick?: (row: RunListItem) => void;
		sortKey?: string;
		sortDir?: 'asc' | 'desc';
		onSort?: (key: RunColumnKey) => void;
	}

	let {
		rows,
		loading = false,
		emptyText = 'No runs match these filters',
		onRowClick,
		sortKey,
		sortDir,
		onSort
	}: Props = $props();

	// Grid tracks are tuned so the status badge column has just enough room for the
	// widest label ("Cancelled") and the rest expand to fill. "Scheduled for" is
	// the Airflow-style logical date — always shown ("—" for manual runs) so you
	// can tell scheduled runs apart without drilling into a schedule.
	const columns: {
		key: RunColumnKey;
		label: string;
		gridTrack?: string;
		sortable?: boolean;
	}[] = [
		{ key: 'id', label: 'ID', gridTrack: '120px' },
		{ key: 'kind', label: 'Kind', gridTrack: '80px', sortable: true },
		{ key: 'script_path', label: 'Script', gridTrack: 'minmax(0,2fr)', sortable: true },
		{ key: 'status', label: 'Status', gridTrack: '104px' },
		{ key: 'scheduled_time', label: 'Scheduled for', gridTrack: '200px', sortable: true },
		{ key: 'created_by', label: 'Created by', gridTrack: 'minmax(0,1.3fr)', sortable: true },
		{ key: 'duration_ms', label: 'Duration', gridTrack: '88px', sortable: true },
		{ key: 'created_at', label: 'Created', gridTrack: '104px', sortable: true }
	];

	async function copyId(id: string, e: MouseEvent): Promise<void> {
		e.stopPropagation();
		try {
			await navigator.clipboard.writeText(id);
		} catch {
			// clipboard unavailable or permission denied — no-op
		}
	}
</script>

<DataTable {rows} {columns} {loading} {emptyText} {onRowClick} {sortKey} {sortDir} {onSort}>
	{#snippet cell({ row, column })}
		{#if column.key === 'id'}
			<div class="group flex items-center gap-1 overflow-hidden">
				<code class="min-w-0 truncate font-mono text-xs text-text-secondary" title={row.id}>
					{row.id.slice(0, 8)}
				</code>
				<button
					type="button"
					class="shrink-0 rounded p-0.5 text-text-tertiary opacity-0 transition-opacity hover:bg-surface-alt hover:text-text group-hover:opacity-100"
					title="Copy full ID"
					aria-label="Copy full ID"
					onclick={(e) => void copyId(row.id, e)}
				>
					<Copy size={10} />
				</button>
			</div>
		{:else if column.key === 'kind'}
			<Badge variant="ghost">{row.kind}</Badge>
		{:else if column.key === 'script_path'}
			<div class="flex min-w-0 flex-col">
				<span class="truncate text-text" title={row.script_path ?? ''}>
					{row.script_path ?? '—'}
				</span>
				{#if row.flow_path}
					<span
						class="truncate text-xs text-text-tertiary"
						title="From flow {row.flow_path}{row.flow_step_id ? ` · node ${row.flow_step_id}` : ''}"
					>
						↳ flow: {row.flow_path}{#if row.flow_step_id} · {row.flow_step_id}{/if}
					</span>
				{/if}
			</div>
		{:else if column.key === 'status'}
			<RunStatusBadge status={row.status} uniform />
		{:else if column.key === 'created_by'}
			<span class="truncate text-text-secondary" title={row.created_by}>
				{row.created_by}
			</span>
		{:else if column.key === 'duration_ms'}
			<span class="font-mono text-xs text-text-secondary">
				{formatDuration(row.duration_ms)}
			</span>
		{:else if column.key === 'scheduled_time'}
			{#if row.scheduled_time}
				<time
					class="font-mono text-xs text-text-secondary"
					datetime={row.scheduled_time}
					title="Scheduled slot (logical date): {row.scheduled_time}"
				>
					{formatAbsolute(row.scheduled_time, displayTz.value)}
				</time>
			{:else}
				<span class="text-xs text-text-tertiary">—</span>
			{/if}
		{:else if column.key === 'created_at'}
			<time
				class="text-xs text-text-tertiary"
				datetime={row.created_at}
				title={row.created_at}
			>
				{formatRelative(row.created_at)}
			</time>
		{/if}
	{/snippet}
</DataTable>
