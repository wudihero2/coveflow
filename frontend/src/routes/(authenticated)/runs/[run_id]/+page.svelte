<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { getContext } from 'svelte';
	import { Activity, ArrowLeft, Ban, FileCode, RefreshCw, RotateCw, Square, Check, X } from '@lucide/svelte';

	import Alert from '$lib/components/common/Alert.svelte';
	import Button from '$lib/components/common/Button.svelte';
	import IconButton from '$lib/components/common/IconButton.svelte';
	import ResizableSplitPane from '$lib/components/common/ResizableSplitPane.svelte';
	import LogViewer from '$lib/components/runs/LogViewer.svelte';
	import RunCodePanel from '$lib/components/runs/RunCodePanel.svelte';
	import RunResultPanel from '$lib/components/runs/RunResultPanel.svelte';
	import RunStatusBadge from '$lib/components/runs/RunStatusBadge.svelte';
	import RunMetadataPanel from '$lib/components/runs/RunMetadataPanel.svelte';
	import RunContextPanel from '$lib/components/runs/RunContextPanel.svelte';
	import FlowStatusPanel from '$lib/components/runs/FlowStatusPanel.svelte';
	import FlowRunGraph from '$lib/components/runs/FlowRunGraph.svelte';
	import CancelRunModal from '$lib/components/runs/CancelRunModal.svelte';
	import RerunModal from '$lib/components/runs/RerunModal.svelte';
	import MarkRunModal from '$lib/components/runs/MarkRunModal.svelte';
	import { api, ApiClientError } from '$lib/services/api';
	import { useWorkspaceLoader } from '$lib/services/workspace-loader.svelte';
	import { workspace } from '$lib/stores/workspace.svelte';
	import { toastError, toastSuccess } from '$lib/toast';
	import type { RunResultEvent, RunStatus, ScriptListItem, WorkspaceRole } from '$lib/types';

	let runId = $derived(page.params.run_id ?? '');

	// Where "back" goes: callers (e.g. the flow run-in-place view, when you click a
	// node) can pass `?from=<internal path>` so back returns there instead of the
	// generic runs list. Only honour same-origin internal paths.
	const backTo = $derived.by(() => {
		const f = page.url.searchParams.get('from');
		return f && f.startsWith('/') && !f.startsWith('//') ? f : null;
	});
	const backLabel = $derived(backTo?.startsWith('/flows/') ? 'Back to flow' : 'Back to runs');
	function goBack(): void {
		void goto(backTo ?? '/runs');
	}

	// Admin role is workspace-scoped — provided by (authenticated)/+layout via context.
	const roleCtx =
		getContext<() => { role: WorkspaceRole | null; roleLoaded: boolean; roleError: boolean }>(
			'auth:role'
		);
	let isAdmin = $derived(roleCtx?.().role === 'admin');

	// -- Run state ------------------------------------------------------------

	// keepData: a reload (Refresh, or handleResult on live completion) must not
	// blank `run` to null — that would unmount RunCodePanel and tear down/recreate
	// its Monaco editor and re-fire the script fetch. Retaining the previous run
	// keeps the panes mounted across the refetch.
	const runLoader = useWorkspaceLoader((ws) => ws.getRun(runId), {
		key: () => runId,
		enabled: () => !!runId,
		keepData: true
	});

	let run = $derived(runLoader.data);
	let status = $derived<RunStatus | null>(run?.status ?? null);
	let inFlight = $derived(status === 'queued' || status === 'running');
	let markedSuccess = $derived(status === 'success');

	// -- Flow runs: render the DAG instead of logs/code --------------------
	const isFlow = $derived(run?.kind === 'flow' || run?.kind === 'flow_preview');

	// Scripts (for flow node titles), fetched once when viewing a flow run.
	let scripts = $state<ScriptListItem[]>([]);
	let scriptsFetched = false;
	$effect(() => {
		if (isFlow && !scriptsFetched) {
			scriptsFetched = true;
			void api
				.forWorkspace(workspace.id)
				.listScripts()
				.then((s) => {
					scripts = s;
				})
				.catch(() => {
					/* node titles fall back to ids */
				});
		}
	});
	const scriptsById = $derived(Object.fromEntries(scripts.map((s) => [s.script_id, s])));
	function scriptLabel(id: string): string | undefined {
		return scriptsById[id]?.path;
	}

	// The DAG replaces the log stream that would otherwise drive live updates, so
	// poll the run while a flow is still in flight (stops once terminal).
	$effect(() => {
		if (isFlow && inFlight) {
			const t = setInterval(() => runLoader.reload(), 1000);
			return () => clearInterval(t);
		}
	});

	// Surface the flow's final result (the log stream isn't mounted for flows).
	$effect(() => {
		if (isFlow && run && !inFlight) {
			resultPanel = { visible: true, success: status === 'success', result: run.result };
		}
	});

	// Right-pane tab: Logs (default) / Code. Logs stay the default because
	// the run page's primary purpose is observing execution; users click Code
	// only when investigating "what actually ran".
	type RightTab = 'logs' | 'code';
	let activeTab = $state<RightTab>('logs');

	// Lazy-mount the Code tab: RunCodePanel pulls in Monaco and fires a
	// script-by-path fetch on mount, so don't pay that for users who only read
	// logs. Latch true on first activation and stay mounted thereafter for
	// instant tab switching.
	let codeTabEverShown = $state(false);

	// Single entry point for tab activation (click + keyboard) so the lazy-mount
	// latch is set on every path without a write-only $effect.
	function selectTab(tab: RightTab): void {
		activeTab = tab;
		if (tab === 'code') codeTabEverShown = true;
	}

	// ARIA tablist standard pattern: arrow keys move focus + selection.
	function onTabKeydown(e: KeyboardEvent): void {
		const tabs: RightTab[] = ['logs', 'code'];
		const currentIndex = tabs.indexOf(activeTab);
		let nextIndex = currentIndex;
		switch (e.key) {
			case 'ArrowRight':
				nextIndex = Math.min(tabs.length - 1, currentIndex + 1);
				break;
			case 'ArrowLeft':
				nextIndex = Math.max(0, currentIndex - 1);
				break;
			case 'Home':
				nextIndex = 0;
				break;
			case 'End':
				nextIndex = tabs.length - 1;
				break;
			default:
				return;
		}
		if (nextIndex !== currentIndex) {
			e.preventDefault();
			selectTab(tabs[nextIndex]);
			document.getElementById(`run-tab-${activeTab}`)?.focus();
		}
	}

	// -- Result panel — populated either from the loader (already-finished run)
	// or from LogViewer's onResult callback while watching a live run.
	type ResultPanel = { visible: false } | { visible: true; success: boolean; result: unknown };
	let resultPanel = $state<ResultPanel>({ visible: false });
	let resultExpanded = $state(true);

	// When the loader resolves on an already-completed run, seed the result
	// panel from `run.result` so the user sees the payload without waiting for
	// the LogViewer to call back. We key on run.id so rerun navigations reset.
	let lastSeededRunId: string | null = null;
	$effect(() => {
		if (!run) return;
		if (status === 'success' || status === 'failure' || status === 'cancelled') {
			// Seed from run.result only when this run hasn't already revealed a
			// panel. handleResult reveals the panel from the SSE payload while the
			// run is still in-flight and then reloads; once that reload lands as
			// terminal, re-seeding from run.result would overwrite (and possibly
			// flash empty, if the result column commits after the status) the
			// payload the LogViewer just showed. Still seed when the panel is
			// hidden — a run that completed without an SSE result event needs it.
			if (run.id !== lastSeededRunId || !resultPanel.visible) {
				resultPanel = {
					visible: true,
					success: status === 'success',
					result: run.result
				};
			}
			lastSeededRunId = run.id;
		} else if (run.id !== lastSeededRunId) {
			// A genuinely new in-flight/queued run starts hidden. Don't hide on a
			// mere re-fetch of the same run (e.g. handleResult's reload landing as
			// 'running' before the backend commits terminal status) — that would
			// flash away the panel the LogViewer just revealed.
			resultPanel = { visible: false };
			lastSeededRunId = run.id;
		}
	});

	// -- Modals --------------------------------------------------------------

	let cancelOpen = $state(false);
	let rerunOpen = $state(false);
	let markOpen = $state(false);
	let markVariant = $state<'success' | 'failure'>('success');

	// -- Action handlers -----------------------------------------------------

	async function confirmCancel(reason: string): Promise<void> {
		const wsId = workspace.id;
		try {
			await api.forWorkspace(wsId).cancelRun(runId, { reason: reason || undefined });
			if (workspace.id !== wsId) return;
			toastSuccess('Cancel requested');
			cancelOpen = false;
			runLoader.reload();
		} catch (e) {
			toastError(
				e instanceof ApiClientError ? `${e.status}: ${e.body || e.message}` : String(e)
			);
		}
	}

	async function confirmRerun(useLatest: boolean): Promise<void> {
		const wsId = workspace.id;
		try {
			const { id } = await api
				.forWorkspace(wsId)
				.rerun(runId, { use_latest_version: useLatest });
			if (workspace.id !== wsId) return;
			toastSuccess(`Started ${id.slice(0, 8)}`);
			rerunOpen = false;
			void goto(`/runs/${id}`);
		} catch (e) {
			toastError(
				e instanceof ApiClientError ? `${e.status}: ${e.body || e.message}` : String(e)
			);
		}
	}

	async function confirmMark(payload: { reason: string; result: unknown }): Promise<void> {
		const wsId = workspace.id;
		const wsApi = api.forWorkspace(wsId);
		const method = markVariant === 'success' ? 'markSuccess' : 'markFail';
		try {
			await wsApi[method](runId, {
				reason: payload.reason || undefined,
				result: payload.result ?? undefined
			});
			if (workspace.id !== wsId) return;
			toastSuccess(markVariant === 'success' ? 'Marked as success' : 'Marked as failure');
			markOpen = false;
			runLoader.reload();
		} catch (e) {
			toastError(
				e instanceof ApiClientError ? `${e.status}: ${e.body || e.message}` : String(e)
			);
		}
	}

	function handleResult(event: RunResultEvent): void {
		resultPanel = { visible: true, success: event.success, result: event.result };
		resultExpanded = true;
		// Only reload when the run is still in-flight so the status badge and
		// action buttons update (queued/running → success/failure). Reloading a
		// run that's already terminal would synchronously null out `run` (execute
		// sets data=null before the await), causing the LogViewer to unmount and
		// restart its poll — creating an infinite cycle for historical runs.
		if (run?.status === 'queued' || run?.status === 'running') {
			runLoader.reload();
		}
	}

	function handleLogError(message: string): void {
		toastError(`Log stream error: ${message}`);
	}

	function openMark(variant: 'success' | 'failure'): void {
		markVariant = variant;
		markOpen = true;
	}
</script>

<svelte:head>
	<title>{run ? `Run ${runId.slice(0, 8)}` : 'Run'} | CoveFlow</title>
</svelte:head>

<div class="flex h-svh flex-col lg:h-svh max-lg:h-[calc(100svh-48px)]">
	<!--
		Top toolbar grouped into 3 visual clusters separated by 1px dividers so
		the eye instantly tells primary actions apart from rare admin overrides:
			[back · run id · status]   [refresh]   [primary action]  │  [admin]
	-->
	<div class="flex items-center gap-3 border-b border-border px-4 py-2.5">
		<IconButton onclick={goBack} aria-label={backLabel}>
			<ArrowLeft size={16} />
		</IconButton>

		<div class="flex items-center gap-2 text-sm">
			<span class="text-text-tertiary">Run</span>
			<code class="rounded bg-surface-alt px-1.5 py-0.5 font-mono text-text" title={runId}>
				{runId.slice(0, 8)}
			</code>
			{#if status}
				<RunStatusBadge {status} />
			{/if}
		</div>

		<div class="flex-1"></div>

		<IconButton
			aria-label="Refresh run"
			title="Refresh"
			onclick={() => runLoader.reload()}
			disabled={runLoader.loading}
		>
			<RefreshCw size={14} />
		</IconButton>

		{#if inFlight}
			<Button variant="secondary" size="sm" onclick={() => (cancelOpen = true)}>
				<Ban size={14} />
				Cancel
			</Button>
		{/if}
		{#if run && (!inFlight || isAdmin)}
			<Button variant="secondary" size="sm" onclick={() => (rerunOpen = true)}>
				<RotateCw size={14} />
				Rerun
			</Button>
		{/if}

		{#if isAdmin && run}
			<!-- Visual separator: admin overrides are rare, dangerous, and should
				 feel "off to the side" rather than crammed next to primary action. -->
			<div class="mx-1 h-5 w-px bg-border" aria-hidden="true"></div>
			<Button variant="ghost" size="sm" onclick={() => openMark('success')}>
				<Check size={14} class="text-success" />
				Mark success
			</Button>
			<Button variant="ghost" size="sm" onclick={() => openMark('failure')}>
				<X size={14} class="text-error" />
				Mark fail
			</Button>
		{/if}
	</div>

	<!--
		Audit strip: shows who cancelled / marked this run and any reason text.
		Sits immediately below the status badge so the verdict and its rationale
		read as one unit. Each line truncates at viewport width and reveals the
		full text in a native tooltip.
	-->
	{#if run && (run.canceled_by || run.marked_by)}
		<!--
			markedSuccess: when `marked_by` is set, the backend's derive_status
			guarantees `status` is either 'success' or 'failure' (mark override
			takes precedence over cancel). Comparing to 'success' is therefore
			safe — never falsely reports "marked failure" for a queued/running
			run because marked_by can only be set on completed runs.
		-->
		<div
			class="flex flex-col gap-0.5 border-b border-border bg-surface-alt/40 px-4 py-1.5 text-xs text-text-secondary"
			role="status"
			aria-label="Run audit info"
		>
			{#if run.canceled_by}
				<div class="flex items-center gap-1.5 truncate" title={run.canceled_reason ?? undefined}>
					<Ban size={12} class="shrink-0 text-text-tertiary" aria-hidden="true" />
					<span class="truncate">
						<span class="font-medium text-text">Cancelled by {run.canceled_by}</span>
						{#if run.canceled_reason}: {run.canceled_reason}{/if}
					</span>
				</div>
			{/if}
			{#if run.marked_by}
				<div class="flex items-center gap-1.5 truncate" title={run.mark_reason ?? undefined}>
					{#if markedSuccess}
						<Check size={12} class="shrink-0 text-success" aria-hidden="true" />
					{:else}
						<X size={12} class="shrink-0 text-error" aria-hidden="true" />
					{/if}
					<span class="truncate">
						<span class="font-medium text-text">
							Marked {markedSuccess ? 'success' : 'failure'} by {run.marked_by}
						</span>
						{#if run.mark_reason}: {run.mark_reason}{/if}
					</span>
				</div>
			{/if}
		</div>
	{/if}

	<!--
		Body: split metadata + logs.
		Wrapper is `flex flex-col` so ResizableSplitPane's `flex-1` can stretch
		to fill the remaining viewport height. A plain `block` wrapper would
		collapse the pane to content height and leave a tall empty band below.
	-->
	<div class="flex min-h-0 flex-1 flex-col">
		<!--
			Gate the full-screen loading/error states on `!run`: the loader keeps the
			previous run visible across a reload (keepData), so once we have a run we
			render the panes and let the refetch happen underneath rather than
			tearing the whole body (and Monaco) down. A reload error with a run still
			present is surfaced via toasts (LogViewer) / the disabled Refresh button,
			not by replacing the page.
		-->
		{#if runLoader.loading && !run}
			<div class="flex h-full items-center justify-center text-sm text-text-tertiary">
				Loading run…
			</div>
		{:else if runLoader.error && !run}
			{#if /^404\b/.test(runLoader.error)}
				<div class="flex h-full flex-col items-center justify-center gap-3 text-sm text-text-tertiary">
					<Square size={20} />
					<span>Run not found</span>
					<Button variant="ghost" size="sm" onclick={goBack}>← {backLabel}</Button>
				</div>
			{:else}
				<div class="p-4">
					<Alert variant="error">{runLoader.error}</Alert>
				</div>
			{/if}
		{:else if !run}
			<div class="flex h-full flex-col items-center justify-center gap-3 text-sm text-text-tertiary">
				<Square size={20} />
				<span>Run not found</span>
				<Button variant="ghost" size="sm" onclick={goBack}>{backLabel}</Button>
			</div>
		{:else}
			{#snippet leftPane()}
				<div class="min-h-0 flex-1 overflow-y-auto bg-surface">
					<RunMetadataPanel {run} />
					<RunContextPanel context={run.context} />
				</div>
			{/snippet}
			{#snippet rightPane()}
				{#if isFlow && run?.flow_value}
					<!-- Flow run: the DAG replaces logs/code. -->
					<FlowRunGraph
							spec={run.flow_value}
							flowStatus={run.flow_status}
							scriptLabel={scripts.length ? scriptLabel : undefined}
						/>
				{:else if isFlow}
					<!-- Flow without a stored spec (older runs): flat status fallback. -->
					<div class="min-h-0 flex-1 overflow-y-auto bg-surface">
						{#if run?.flow_status}
							<FlowStatusPanel status={run.flow_status} />
						{/if}
					</div>
				{:else}
				<!--
					Right pane: Logs / Code tabs sit above; RunResultPanel stays
					below the active tab content so the verdict is visible regardless
					of which tab the user is on. Both tab panels stay mounted (hidden
					via `class="hidden"`) so LogViewer keeps polling in the background
					and switching tabs is instant.
				-->
				<div
					role="tablist"
					aria-label="Right panel sections"
					class="flex border-b border-border bg-surface-raised text-sm"
				>
					<button
						type="button"
						id="run-tab-logs"
						role="tab"
						aria-selected={activeTab === 'logs'}
						aria-controls="run-panel-logs"
						tabindex={activeTab === 'logs' ? 0 : -1}
						class="-mb-px inline-flex items-center gap-1.5 border-b-2 px-4 py-1.5 {activeTab === 'logs'
							? 'border-info text-text'
							: 'border-transparent text-text-tertiary hover:text-text'}"
						onclick={() => selectTab('logs')}
						onkeydown={onTabKeydown}
					>
						<Activity size={12} aria-hidden="true" />
						Logs
					</button>
					<button
						type="button"
						id="run-tab-code"
						role="tab"
						aria-selected={activeTab === 'code'}
						aria-controls="run-panel-code"
						tabindex={activeTab === 'code' ? 0 : -1}
						class="-mb-px inline-flex items-center gap-1.5 border-b-2 px-4 py-1.5 {activeTab === 'code'
							? 'border-info text-text'
							: 'border-transparent text-text-tertiary hover:text-text'}"
						onclick={() => selectTab('code')}
						onkeydown={onTabKeydown}
					>
						<FileCode size={12} aria-hidden="true" />
						Code
					</button>
				</div>

				<div
					id="run-panel-logs"
					role="tabpanel"
					aria-labelledby="run-tab-logs"
					class="flex min-h-0 flex-1 flex-col {activeTab === 'logs' ? '' : 'hidden'}"
				>
					<LogViewer
						workspaceId={workspace.id}
						runId={runId}
						onResult={handleResult}
						onError={handleLogError}
						class="min-h-0 flex-1"
					/>
				</div>

				<div
					id="run-panel-code"
					role="tabpanel"
					aria-labelledby="run-tab-code"
					class="flex min-h-0 flex-1 flex-col {activeTab === 'code' ? '' : 'hidden'}"
				>
					{#if run && codeTabEverShown}
						<RunCodePanel {run} workspaceId={workspace.id} />
					{/if}
				</div>
				{/if}

				{#if resultPanel.visible}
					<RunResultPanel
						success={resultPanel.success}
						result={resultPanel.result}
						expanded={resultExpanded}
						onToggle={() => (resultExpanded = !resultExpanded)}
					/>
				{/if}
			{/snippet}
			<ResizableSplitPane
				primary={leftPane}
				secondary={rightPane}
				defaultPercent={30}
				storageKey="run-detail-split"
				ariaLabel="Resize metadata and log panels"
			/>
		{/if}
	</div>
</div>

<!-- Action modals -->
<CancelRunModal
	bind:open={cancelOpen}
	{runId}
	onConfirm={confirmCancel}
	onCancel={() => (cancelOpen = false)}
/>

<RerunModal
	bind:open={rerunOpen}
	{runId}
	onConfirm={confirmRerun}
	onCancel={() => (rerunOpen = false)}
/>

<MarkRunModal
	bind:open={markOpen}
	variant={markVariant}
	{runId}
	onConfirm={confirmMark}
	onCancel={() => (markOpen = false)}
/>
