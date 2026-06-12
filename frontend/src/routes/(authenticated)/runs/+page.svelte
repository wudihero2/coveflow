<script lang="ts">
	import { RefreshCw, X } from '@lucide/svelte';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';

	import Alert from '$lib/components/common/Alert.svelte';
	import Button from '$lib/components/common/Button.svelte';
	import PageFrame from '$lib/components/common/PageFrame.svelte';
	import RunFilters from '$lib/components/runs/RunFilters.svelte';
	import RunTable from '$lib/components/runs/RunTable.svelte';
	import { api, ApiClientError } from '$lib/services/api';
	import { workspace } from '$lib/stores/workspace.svelte';
	import { toastError } from '$lib/toast';
	import type { RunListItem } from '$lib/types';

	const PAGE_SIZE = 20;

	// -- Filter + pagination state -------------------------------------------

	let statusFilter = $state('');
	let kindFilter = $state('');
	let createdByFilter = $state('');
	let scriptPathFilter = $state('');
	// Server-side sort (the list is paginated, so sorting must be done in the DB).
	let sortBy = $state('created_at');
	let sortOrder = $state<'asc' | 'desc'>('desc');
	function onSort(key: string): void {
		if (sortBy === key) {
			sortOrder = sortOrder === 'asc' ? 'desc' : 'asc';
		} else {
			sortBy = key;
			sortOrder = 'desc';
		}
	}
	let timeRange = $state<{ after?: number; before?: number }>({});
	// Optional ?schedule_id=… (set when arriving from a schedule's history button).
	const scheduleId = $derived(page.url.searchParams.get('schedule_id') ?? undefined);

	let rows = $state.raw<RunListItem[]>([]);
	let hasMore = $state(false);
	let loading = $state(true);
	let loadingMore = $state(false);
	let loadError = $state('');
	let loadGeneration = 0;

	function buildQuery(offset: number) {
		return {
			status: statusFilter || undefined,
			kind: kindFilter || undefined,
			created_by: createdByFilter.trim() || undefined,
			script_path: scriptPathFilter.trim() || undefined,
			created_after_ms: timeRange.after,
			created_before_ms: timeRange.before,
			schedule_id: scheduleId,
			sort: sortBy,
			order: sortOrder,
			limit: PAGE_SIZE,
			offset
		};
	}

	$effect(() => {
		const wsId = workspace.id;
		const filterKey = `${statusFilter}|${kindFilter}|${createdByFilter}|${scriptPathFilter}|${timeRange.after}|${timeRange.before}|${scheduleId}|${sortBy}|${sortOrder}`;
		void filterKey;
		if (!wsId) return;
		void load(wsId);
	});

	async function load(wsId: string): Promise<void> {
		const gen = ++loadGeneration;
		loading = true;
		loadingMore = false;
		loadError = '';
		rows = [];
		hasMore = false;
		try {
			const response = await api.forWorkspace(wsId).listRuns(buildQuery(0));
			if (gen !== loadGeneration) return;
			rows = response;
			hasMore = response.length === PAGE_SIZE;
		} catch (e) {
			if (gen !== loadGeneration) return;
			loadError =
				e instanceof ApiClientError ? `${e.status}: ${e.body || e.message}` : String(e);
		} finally {
			if (gen === loadGeneration) loading = false;
		}
	}

	async function loadMore(): Promise<void> {
		const gen = loadGeneration;
		if (loadingMore || !hasMore) return;
		loadingMore = true;
		try {
			const response = await api.forWorkspace(workspace.id).listRuns(buildQuery(rows.length));
			if (gen !== loadGeneration) return;
			rows = [...rows, ...response];
			hasMore = response.length === PAGE_SIZE;
		} catch (e) {
			if (gen !== loadGeneration) return;
			toastError(
				e instanceof ApiClientError ? `${e.status}: ${e.body || e.message}` : String(e)
			);
		} finally {
			loadingMore = false;
		}
	}

	function openRun(run: RunListItem): void {
		void goto(`/runs/${run.id}`);
	}
</script>

<svelte:head>
	<title>Runs | CoveFlow</title>
</svelte:head>

<PageFrame title="Runs">
	{#snippet actions()}
		<Button onclick={() => void load(workspace.id)} loading={loading} disabled={loading || loadingMore}>
			<RefreshCw size={16} />
			Refresh
		</Button>
	{/snippet}

	<section class="space-y-4 py-6">
		{#if scheduleId}
			<div
				class="flex items-center justify-between rounded-md border border-accent/30 bg-accent/10 px-3 py-2 text-sm text-text-secondary"
			>
				<span>Showing runs from one schedule.</span>
				<button
					type="button"
					class="inline-flex items-center gap-1 text-accent hover:underline"
					onclick={() => goto('/runs')}
				>
					<X size={14} /> Clear
				</button>
			</div>
		{/if}

		<RunFilters
			bind:status={statusFilter}
			bind:kind={kindFilter}
			bind:createdBy={createdByFilter}
			bind:scriptPath={scriptPathFilter}
			bind:timeRange={timeRange}
		/>

		{#if loadError}
			<Alert variant="error">{loadError}</Alert>
		{/if}

		<RunTable
			{rows}
			{loading}
			onRowClick={openRun}
			sortKey={sortBy}
			sortDir={sortOrder}
			{onSort}
		/>

		{#if hasMore}
			<div class="flex justify-center">
				<Button
					variant="ghost"
					size="sm"
					loading={loadingMore}
					onclick={() => void loadMore()}
				>
					Load {PAGE_SIZE} more
				</Button>
			</div>
		{/if}
	</section>
</PageFrame>
