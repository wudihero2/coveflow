<script lang="ts">
	import { RotateCw, Trash2 } from '@lucide/svelte';

	import DataTable from '$lib/components/common/DataTable.svelte';
	import type { SecretListItem } from '$lib/types';
	import { formatRelative } from '$lib/utils/format-time';

	interface Props {
		secrets: readonly SecretListItem[];
		loading?: boolean;
		onRotate: (path: string) => void;
		onDelete: (item: SecretListItem) => void;
	}

	let { secrets, loading = false, onRotate, onDelete }: Props = $props();

	// DataTable keys must be real row fields, so carry a synthetic `actions`
	// column on the row (the established pattern for action columns).
	type Row = SecretListItem & { actions: string };
	const rows = $derived<Row[]>(secrets.map((s) => ({ ...s, actions: '' })));

	const columns: { key: keyof Row & string; label: string; gridTrack?: string }[] = [
		{ key: 'path', label: 'Key', gridTrack: 'minmax(0,2fr)' },
		{ key: 'description', label: 'Description', gridTrack: 'minmax(0,2fr)' },
		{ key: 'updated_by', label: 'Updated by', gridTrack: 'minmax(0,1.3fr)' },
		{ key: 'updated_at', label: 'Updated', gridTrack: '104px' },
		{ key: 'actions', label: '', gridTrack: '88px' }
	];
</script>

<DataTable {rows} {columns} {loading} emptyText="No secrets yet">
	{#snippet cell({ row, column })}
		{#if column.key === 'path'}
			<code class="truncate font-mono text-sm text-text" title={row.path}>{row.path}</code>
		{:else if column.key === 'description'}
			<span class="truncate text-text-secondary" title={row.description}>
				{row.description || '—'}
			</span>
		{:else if column.key === 'updated_by'}
			<span class="truncate text-text-secondary" title={row.updated_by}>{row.updated_by}</span>
		{:else if column.key === 'updated_at'}
			<time class="text-xs text-text-tertiary" datetime={row.updated_at} title={row.updated_at}>
				{formatRelative(row.updated_at)}
			</time>
		{:else if column.key === 'actions'}
			<div class="flex items-center justify-end gap-1">
				<button
					type="button"
					class="rounded p-1 text-text-tertiary transition hover:bg-surface-alt hover:text-text"
					title="Rotate value"
					aria-label="Rotate value"
					onclick={() => onRotate(row.path)}
				>
					<RotateCw size={14} />
				</button>
				<button
					type="button"
					class="rounded p-1 text-text-tertiary transition hover:bg-surface-alt hover:text-error"
					title="Delete secret"
					aria-label="Delete secret"
					onclick={() => onDelete(row)}
				>
					<Trash2 size={14} />
				</button>
			</div>
		{/if}
	{/snippet}
</DataTable>
