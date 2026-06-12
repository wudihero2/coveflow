<script lang="ts">
	import { ArrowLeft, RefreshCw } from '@lucide/svelte';
	import { getContext } from 'svelte';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';

	import Alert from '$lib/components/common/Alert.svelte';
	import Button from '$lib/components/common/Button.svelte';
	import DataTable from '$lib/components/common/DataTable.svelte';
	import PageFrame from '$lib/components/common/PageFrame.svelte';
	import ResourceBar from '$lib/components/common/ResourceBar.svelte';
	import WorkerStatusBadge from '$lib/components/common/WorkerStatusBadge.svelte';
	import { api, ApiClientError } from '$lib/services/api';
	import type { ClusterWorker, ClusterWorkerRun, ClusterWorkerRunsResponse } from '$lib/types';

	// Access (instance-admin only) is enforced by admin/+layout.svelte — the
	// '/admin/cluster' prefix covers this nested route too — so we only read the
	// role state to know when to load (probe done) or warn (probe failed).
	const getAuthRole = getContext<() => { roleLoaded: boolean; roleError: boolean }>('auth:role');

	let worker = $derived(page.params.worker ?? '');

	let runs = $state<ClusterWorkerRunsResponse>({ items: [], has_more: false });
	// Worker capacity/status header, looked up from the cluster workers list. Best
	// effort: a failure here doesn't block the active-runs table below.
	let meta = $state<ClusterWorker | null>(null);
	let loading = $state(false);
	let error = $state<string | null>(null);
	let roleError = $state(false);

	// Generation guard against same-page request races (refresh / param change).
	let gen = 0;
	let loadedKey = '';

	async function reload(): Promise<void> {
		const w = worker;
		if (!w) return;
		const thisGen = ++gen;
		loading = true;
		error = null;
		try {
			const [runsResult, workersResult] = await Promise.allSettled([
				api.cluster.workerRuns(w),
				api.cluster.workers()
			]);
			if (thisGen !== gen) return;
			if (runsResult.status === 'rejected') throw runsResult.reason;
			runs = runsResult.value;
			meta =
				workersResult.status === 'fulfilled'
					? (workersResult.value.find((x) => x.worker === w) ?? null)
					: null;
		} catch (e) {
			if (thisGen !== gen) return;
			error = e instanceof ApiClientError ? e.body || e.message : String(e);
		} finally {
			if (thisGen === gen) loading = false;
		}
	}

	// Load once the role probe resolves, and again whenever the worker param
	// changes (the component is reused across sibling worker routes).
	$effect(() => {
		const { roleLoaded, roleError: re } = getAuthRole();
		roleError = re;
		const w = worker;
		if (!roleLoaded || re || !w || loadedKey === w) return;
		loadedKey = w;
		void reload();
	});

	function elapsed(startedAt?: string): string {
		if (!startedAt) return 'queued';
		const secs = Math.max(0, Math.floor((Date.now() - new Date(startedAt).getTime()) / 1000));
		if (secs < 60) return `${secs}s`;
		if (secs < 3600) return `${Math.floor(secs / 60)}m`;
		return `${Math.floor(secs / 3600)}h${Math.floor((secs % 3600) / 60)}m`;
	}

	const columns = [
		{ key: 'run_id', label: 'Run', gridTrack: 'minmax(280px,2fr)' },
		{ key: 'workspace_id', label: 'Workspace', gridTrack: 'minmax(140px,1fr)' },
		{ key: 'language', label: 'Language', gridTrack: '110px' },
		{ key: 'cpus', label: 'Resources', gridTrack: 'minmax(170px,1fr)' },
		{ key: 'tag', label: 'Tag', gridTrack: '110px' },
		{ key: 'started_at', label: 'Running for', gridTrack: '110px' }
	] satisfies { key: keyof ClusterWorkerRun; label: string; gridTrack?: string }[];
</script>

<svelte:head>
	<title>{meta?.display_name || worker || 'Worker'} | CoveFlow</title>
</svelte:head>

<PageFrame
	title={meta?.display_name || worker || 'Worker'}
	subtitle={meta && meta.display_name !== worker ? worker : 'Active runs on this worker'}
>
	{#snippet actions()}
		<Button variant="ghost" onclick={() => void goto('/admin/cluster')}>
			<ArrowLeft size={16} />
			Back
		</Button>
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
		{#if meta}
			<!-- Worker header: status + live capacity, mirroring the cluster table row. -->
			<div class="mb-4 flex flex-wrap items-center gap-x-6 gap-y-3 rounded-lg border border-border bg-surface-raised px-4 py-3">
				<WorkerStatusBadge status={meta.status} />
				<span class="text-sm text-text-secondary tabular-nums">{meta.running_jobs} running</span>
				<div class="w-40"><ResourceBar used={meta.cpus.used} total={meta.cpus.total} unit="cores" /></div>
				<div class="w-40">
					<ResourceBar used={meta.memory_mb.used / 1024} total={meta.memory_mb.total / 1024} unit="GB" />
				</div>
				<div class="w-40">
					<ResourceBar used={meta.disk_mb.used / 1024} total={meta.disk_mb.total / 1024} unit="GB" />
				</div>
			</div>
		{/if}

		{#if meta?.status === 'stale'}
			<!-- Stale = missed recent heartbeats but not yet reaped, so these runs still
			     read as running even though the worker may already be gone. (Alert has
			     no 'warning' variant, so style a plain notice in the warning color.) -->
			<p
				class="mb-4 rounded-md border border-warning/30 bg-warning/5 px-4 py-3 text-sm text-warning"
			>
				Worker is stale (missed recent heartbeats). These runs will be failed once it passes the
				reap threshold.
			</p>
		{/if}

		{#if runs.has_more}
			<p class="mb-2 text-xs text-text-tertiary">
				Showing the most recent {runs.items.length} — more are running.
			</p>
		{/if}

		<DataTable {columns} rows={runs.items} {loading} emptyText="No active runs.">
			{#snippet cell({ row, column })}
				{#if column.key === 'run_id'}
					<span class="truncate font-mono text-sm text-text">{row.run_id}</span>
				{:else if column.key === 'workspace_id'}
					<span class="truncate text-text-secondary">{row.workspace_id}</span>
				{:else if column.key === 'language'}
					<span class="text-text-secondary">{row.language ?? '—'}</span>
				{:else if column.key === 'cpus'}
					<span class="text-xs text-text-secondary tabular-nums">
						{row.cpus} cpu · {(row.memory_mb / 1024).toFixed(1)} GB · {(row.disk_mb / 1024).toFixed(
							1
						)} GB
					</span>
				{:else if column.key === 'tag'}
					<span class="truncate text-text-secondary">{row.tag}</span>
				{:else if column.key === 'started_at'}
					<span class="tabular-nums text-text-secondary">{elapsed(row.started_at)}</span>
				{/if}
			{/snippet}
		</DataTable>
	{/if}
</PageFrame>
