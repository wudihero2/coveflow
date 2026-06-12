<script lang="ts" generics="Row extends Record<string, any>, Key extends Extract<keyof Row, string> = Extract<keyof Row, string>">
	import { LoaderCircle, ChevronUp, ChevronDown } from '@lucide/svelte';
	import type { Snippet } from 'svelte';

	// Column key points at a field on each row; gridTrack is a CSS grid track value.
	interface Column<K extends string = string> {
		key: K;
		label: string;
		gridTrack?: string;
		/** When true (and `onSort` is provided), this header is clickable to sort. */
		sortable?: boolean;
	}

	interface Props {
		columns: Column<Key>[];
		rows: readonly Row[];
		loading?: boolean;
		emptyText?: string;
		cell?: Snippet<[{ row: Row; column: Column<Key>; value: Row[Key] }]>;
		onRowClick?: (row: Row) => void;
		/** Active sort column + direction. With `onSort`, sortable headers become
		 *  clickable; the parent owns the sort state and re-queries on change. */
		sortKey?: string;
		sortDir?: 'asc' | 'desc';
		onSort?: (key: Key) => void;
	}

	let {
		columns,
		rows,
		loading = false,
		emptyText = 'No data',
		cell,
		onRowClick,
		sortKey,
		sortDir,
		onSort
	}: Props = $props();

	function handleRowKeydown(e: KeyboardEvent, row: Row): void {
		if (onRowClick && (e.key === 'Enter' || e.key === ' ')) {
			e.preventDefault();
			onRowClick(row);
		}
	}

	// Header and rows use the same CSS grid tracks so cells stay aligned.
	const gridTemplate = $derived(
		columns.map((c) => c.gridTrack || 'minmax(0,1fr)').join(' ')
	);
</script>

<table class="w-full overflow-hidden rounded-lg border border-border bg-surface-raised">
	<thead>
		<tr
			class="grid border-b border-border bg-surface-alt px-4 py-3 text-left text-xs font-semibold text-text-tertiary uppercase"
			style:grid-template-columns={gridTemplate}
		>
			{#each columns as col (col.key)}
				{#if col.sortable && onSort}
					<th class="font-semibold">
						<button
							type="button"
							class="inline-flex items-center gap-1 uppercase transition hover:text-text {sortKey ===
							col.key
								? 'text-text'
								: ''}"
							onclick={() => onSort?.(col.key)}
						>
							{col.label}
							{#if sortKey === col.key}
								{#if sortDir === 'asc'}<ChevronUp size={12} />{:else}<ChevronDown size={12} />{/if}
							{/if}
						</button>
					</th>
				{:else}
					<th class="font-semibold">{col.label}</th>
				{/if}
			{/each}
		</tr>
	</thead>

	<tbody>
		{#if loading}
			<tr>
				<td colspan={columns.length}>
					<div class="flex h-40 items-center justify-center gap-2 text-sm text-text-tertiary">
						<LoaderCircle size={16} class="animate-spin" />
						Loading
					</div>
				</td>
			</tr>
		{:else if rows.length === 0}
			<tr>
				<td colspan={columns.length}>
					<div class="flex h-40 items-center justify-center text-sm text-text-tertiary">
						{emptyText}
					</div>
				</td>
			</tr>
		{:else}
			{#each rows as item (item)}
				<tr
					class="grid border-t border-border-subtle px-4 py-3 text-sm transition hover:bg-surface-alt"
					class:cursor-pointer={!!onRowClick}
					style:grid-template-columns={gridTemplate}
					onclick={() => onRowClick?.(item)}
					onkeydown={(e) => handleRowKeydown(e, item)}
					role={onRowClick ? 'button' : undefined}
					tabindex={onRowClick ? 0 : undefined}
				>
					{#each columns as col (col.key)}
						{#if cell}
							<td class="truncate">
								<!-- Optional caller-provided rendering for feature-specific cells. -->
								{@render cell({ row: item, column: col, value: item[col.key] })}
							</td>
						{:else}
							<td class="truncate">{item[col.key] ?? '-'}</td>
						{/if}
					{/each}
				</tr>
			{/each}
		{/if}
	</tbody>
</table>
