<script lang="ts">
	// Feature wrapper around the generic table primitive.
	import DataTable from '$lib/components/common/DataTable.svelte';
	import type { ScriptListItem } from '$lib/types';

	interface Props {
		scripts: ScriptListItem[];
		loading?: boolean;
		onRowClick?: (script: ScriptListItem) => void;
	}

	let { scripts, loading = false, onRowClick }: Props = $props();

	// Column keys must match ScriptListItem fields; satisfies keeps that checked.
	const columns = [
		{ key: 'name', label: 'Name', gridTrack: 'minmax(140px,1fr)' },
		{ key: 'path', label: 'Path', gridTrack: 'minmax(160px,1fr)' },
		{ key: 'language', label: 'Lang', gridTrack: '110px' },
		{ key: 'summary', label: 'Summary', gridTrack: 'minmax(140px,1fr)' },
		{ key: 'created_by', label: 'Created by', gridTrack: '160px' }
	] satisfies { key: keyof ScriptListItem; label: string; gridTrack?: string }[];
</script>

<DataTable {columns} rows={scripts} {loading} emptyText="No scripts yet" {onRowClick}>
	<!-- Custom cell snippet keeps script-specific formatting out of DataTable. -->
	{#snippet cell({ column, value })}
		{#if column.key === 'name'}
			<span class="truncate font-medium text-text">{value}</span>
		{:else if column.key === 'path'}
			<span class="truncate font-mono text-text-secondary">{value}</span>
		{:else if column.key === 'language'}
			<span class="text-text-secondary">{value}</span>
		{:else if column.key === 'summary'}
			<span class="truncate text-text-secondary">{value || '-'}</span>
		{:else}
			<span class="truncate text-text-tertiary">{value}</span>
		{/if}
	{/snippet}
</DataTable>
