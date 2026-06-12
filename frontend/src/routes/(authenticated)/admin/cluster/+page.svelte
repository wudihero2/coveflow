<script lang="ts">
	import { RefreshCw } from '@lucide/svelte';
	import { getContext } from 'svelte';
	import { goto } from '$app/navigation';

	import Alert from '$lib/components/common/Alert.svelte';
	import Button from '$lib/components/common/Button.svelte';
	import DataTable from '$lib/components/common/DataTable.svelte';
	import PageFrame from '$lib/components/common/PageFrame.svelte';
	import ResourceBar from '$lib/components/common/ResourceBar.svelte';
	import WorkerStatusBadge from '$lib/components/common/WorkerStatusBadge.svelte';
	import ClusterSummaryBar from '$lib/components/admin/ClusterSummaryBar.svelte';
	import { api, ApiClientError } from '$lib/services/api';
	import type { ClusterSummary, ClusterWorker } from '$lib/types';

	// Access (instance-admin only) is enforced by admin/+layout.svelte, so this
	// page only mounts for instance admins — no redirect needed here. We still read
	// the role state to know when the probe is done (to load) or failed (to warn).
	const getAuthRole = getContext<
		() => { roleLoaded: boolean; roleError: boolean }
	>('auth:role');

	// Cluster data is global (not workspace-scoped), refreshed manually.
	let summary = $state<ClusterSummary | null>(null);
	let workers = $state<ClusterWorker[]>([]);
	let loading = $state(false);
	let error = $state<string | null>(null);

	let initialLoadDone = false;
	// True when the role probe (/me) itself failed. An instance-admin would
	// otherwise see a silent empty table with no error and no retry path.
	let roleError = $state(false);

	async function reload(): Promise<void> {
		loading = true;
		error = null;
		try {
			const [s, w] = await Promise.all([api.cluster.summary(), api.cluster.workers()]);
			summary = s;
			workers = w;
		} catch (e) {
			error = e instanceof ApiClientError ? e.body || e.message : String(e);
		} finally {
			loading = false;
		}
	}

	// Load once the role probe resolves successfully; surface a probe failure as an
	// error rather than a silent empty table. (Access itself is already gated by
	// the admin layout.)
	$effect(() => {
		const { roleLoaded, roleError: re } = getAuthRole();
		roleError = re;
		if (roleLoaded && !re && !initialLoadDone) {
			initialLoadDone = true;
			void reload();
		}
	});

	function openWorker(w: ClusterWorker): void {
		void goto(`/admin/cluster/${encodeURIComponent(w.worker)}`);
	}

	const columns = [
		{ key: 'worker', label: 'Worker', gridTrack: 'minmax(120px,1.5fr)' },
		{ key: 'tags', label: 'Tags', gridTrack: 'minmax(100px,1fr)' },
		{ key: 'cpus', label: 'CPU', gridTrack: 'minmax(130px,1fr)' },
		{ key: 'memory_mb', label: 'RAM', gridTrack: 'minmax(130px,1fr)' },
		{ key: 'disk_mb', label: 'Disk', gridTrack: 'minmax(130px,1fr)' },
		{ key: 'running_jobs', label: 'Runs', gridTrack: '70px' },
		{ key: 'status', label: 'Status', gridTrack: '110px' }
	] satisfies { key: keyof ClusterWorker; label: string; gridTrack?: string }[];
</script>

<svelte:head>
	<title>Cluster | CoveFlow</title>
</svelte:head>

<PageFrame title="Cluster" subtitle="Worker capacity across the instance">
	{#snippet actions()}
		<Button variant="secondary" onclick={reload} disabled={loading}>
			<RefreshCw size={16} class={loading ? 'animate-spin' : ''} />
			Refresh
		</Button>
	{/snippet}

	{#if roleError}
		<Alert variant="error">Could not verify your admin role. Please refresh the page.</Alert>
	{:else if error}
		<Alert variant="error">{error}</Alert>
	{:else}
		<ClusterSummaryBar {summary} />
		<DataTable {columns} rows={workers} {loading} emptyText="No workers" onRowClick={openWorker}>
			{#snippet cell({ row, column })}
				{#if column.key === 'worker'}
					<div class="flex min-w-0 flex-col">
						<span class="truncate font-medium text-text">{row.display_name}</span>
						<!-- Unique per-process identity: lets operators tell two live
						     processes (or a just-restarted one) with the same name apart. -->
						<span class="truncate font-mono text-xs text-text-tertiary" title={row.worker}>
							{row.worker}
						</span>
					</div>
				{:else if column.key === 'tags'}
					<div class="flex flex-wrap gap-1">
						<!-- Index key: tags come from worker_ping.tags (TEXT[]) with no
						     uniqueness, so duplicates would crash an each keyed by value. -->
						{#each row.tags as tag, i (i)}
							<span class="rounded-full bg-surface-alt px-2 py-0.5 text-xs text-text-secondary"
								>{tag}</span
							>
						{/each}
					</div>
				{:else if column.key === 'cpus'}
					<ResourceBar used={row.cpus.used} total={row.cpus.total} unit="cores" />
				{:else if column.key === 'memory_mb'}
					<ResourceBar used={row.memory_mb.used / 1024} total={row.memory_mb.total / 1024} unit="GB" />
				{:else if column.key === 'disk_mb'}
					<ResourceBar used={row.disk_mb.used / 1024} total={row.disk_mb.total / 1024} unit="GB" />
				{:else if column.key === 'status'}
					<WorkerStatusBadge status={row.status} />
				{:else}
					<span class="tabular-nums">{row.running_jobs}</span>
				{/if}
			{/snippet}
		</DataTable>
	{/if}
</PageFrame>
