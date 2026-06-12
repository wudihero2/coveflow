<script lang="ts">
	import { beforeNavigate, goto } from '$app/navigation';
	import { page } from '$app/state';
	import { Activity, ChevronDown, History } from '@lucide/svelte';

	import ScriptEditContent from '$lib/components/scripts/ScriptEditContent.svelte';
	import ScriptDraftNotice from '$lib/components/scripts/ScriptDraftNotice.svelte';
	import ScriptEditShell from '$lib/components/scripts/ScriptEditShell.svelte';
	import ScriptEditor from '$lib/components/scripts/ScriptEditor.svelte';
	import ScriptHistory from '$lib/components/scripts/ScriptHistory.svelte';
	import VersionDiffModal from '$lib/components/scripts/VersionDiffModal.svelte';
	import RestoreConfirmModal from '$lib/components/scripts/RestoreConfirmModal.svelte';
	import { DEFAULT_PYTHON_RUNTIME } from '$lib/components/scripts/editor/languages';
	import LogViewer from '$lib/components/runs/LogViewer.svelte';
	import RunResultPanel from '$lib/components/runs/RunResultPanel.svelte';
	import RunOptionsPopover from '$lib/components/runs/RunOptionsPopover.svelte';
	import { runOptions } from '$lib/components/runs/run-options/store.svelte';
	import Button from '$lib/components/common/Button.svelte';
	import { api, ApiClientError } from '$lib/services/api';
	import {
		draftDiffersFromBaseline,
		readScriptDraft,
		removeScriptDraft,
		writeScriptDraft,
		type ScriptDraft
	} from '$lib/services/script-drafts';
	import { useWorkspaceLoader } from '$lib/services/workspace-loader.svelte';
	import { encodePath } from '$lib/services/url';
	import { confirmNavigation } from '$lib/stores/confirm.svelte';
	import { workspace } from '$lib/stores/workspace.svelte';
	import { toastError, toastInfo, toastSuccess } from '$lib/toast';
	import type { RunResultEvent, ScriptLang, ScriptResponse } from '$lib/types';

	let scriptPath = $derived(page.params.path ?? '');

	// -- Versioned view ("Edit ↗" deep link from a past run) ------------------
	//
	// URL `?version=<hash>` means "load this exact prior version, readonly".
	// Three resolutions:
	//   - Hash equals current HEAD → strip the param, fall through to normal editable mode
	//     (Q8 — `?version` becomes a no-op when it points at HEAD).
	//   - Hash exists in history → show it readonly with a "Switch to current" banner
	//     (Q6 — Edit takes you to that exact version, but you must opt back to HEAD to mutate).
	//   - Hash 404 → fall back to HEAD and surface a dismissible warning banner
	//     (Q7 — "Version v… no longer available, showing current").
	let versionParam = $derived(page.url.searchParams.get('version'));

	type VersionState =
		| { kind: 'idle' }
		| { kind: 'loading' }
		| { kind: 'preview'; script: ScriptResponse }
		| { kind: 'fallback' } // hash 404; the version is genuinely gone
		| { kind: 'error' }; // non-404 fetch failure; version may be fine — offer retry

	let versionState = $state<VersionState>({ kind: 'idle' });
	let versionBannerDismissed = $state(false);
	let lastVersionFetchKey = $state<string | null>(null);
	// Monotonic guard so only the latest version fetch may write versionState —
	// covers a Retry fired while an earlier request is still in flight.
	let versionFetchEpoch = 0;

	const scriptLoader = useWorkspaceLoader((ws) => ws.getScriptByPath(scriptPath), {
		key: () => scriptPath,
		enabled: () => !!scriptPath
	});

	$effect(() => {
		const path = scriptPath;
		const hash = versionParam;
		const headHash = scriptLoader.data?.hash;
		const fetchKey = `${path}::${hash ?? ''}`;

		// No version param → idle
		if (!hash) {
			versionState = { kind: 'idle' };
			lastVersionFetchKey = null;
			versionBannerDismissed = false;
			return;
		}

		// Wait until we know HEAD before deciding anything
		if (!headHash) return;

		// Q8: hash == HEAD → strip param, normal editable mode
		if (hash === headHash) {
			void goto(`/scripts/edit/${encodePath(path)}`, {
				replaceState: true,
				noScroll: true,
				keepFocus: true
			});
			return;
		}

		// Skip if already in-flight for this combo
		if (lastVersionFetchKey === fetchKey) return;
		lastVersionFetchKey = fetchKey;
		versionState = { kind: 'loading' };
		versionBannerDismissed = false;

		const wsId = workspace.id;
		const epoch = ++versionFetchEpoch;
		void (async () => {
			try {
				const script = await api.forWorkspace(wsId).getScriptByHash(hash);
				// Guard against a stale resolve: re-check workspace/path/hash AND the
				// epoch, so a slower fetch (or one superseded by Retry) can't overwrite.
				if (
					epoch !== versionFetchEpoch ||
					workspace.id !== wsId ||
					scriptPath !== path ||
					versionParam !== hash
				)
					return;
				// Q7 safety: a hash can resolve but belong to a different path (data
				// corruption or shared-hash edge case). Treat as not found.
				if (script.path !== path) {
					versionState = { kind: 'fallback' };
					return;
				}
				versionState = { kind: 'preview', script };
			} catch (e) {
				if (
					epoch !== versionFetchEpoch ||
					workspace.id !== wsId ||
					scriptPath !== path ||
					versionParam !== hash
				)
					return;
				if (e instanceof ApiClientError && e.status === 404) {
					// The version is genuinely gone → fall back to HEAD with a notice.
					versionState = { kind: 'fallback' };
				} else {
					// Non-404 (network / 500 / auth): the version may be fine, the fetch
					// just failed. Don't claim it's gone; offer a retry.
					versionState = { kind: 'error' };
				}
			}
		})();
	});

	let viewMode = $derived<'editable' | 'preview' | 'preview-loading'>(
		versionState.kind === 'preview'
			? 'preview'
			: versionState.kind === 'loading'
				? 'preview-loading'
				: 'editable'
	);
	let isReadonly = $derived(viewMode !== 'editable');

	function exitPreview(): void {
		versionState = { kind: 'idle' };
		versionBannerDismissed = false;
		void goto(`/scripts/edit/${encodePath(scriptPath)}`, {
			replaceState: true,
			keepFocus: true
		});
	}

	// Retry a failed (non-404) version load: clearing the dedupe key re-triggers
	// the version effect for the same params.
	function retryVersionLoad(): void {
		lastVersionFetchKey = null;
	}

	const teamsLoader = useWorkspaceLoader((ws) => ws.listTeams());

	// In preview mode, "loaded" reflects the pinned version; otherwise HEAD.
	// Save still always targets the HEAD path, but Save is disabled while readonly.
	let previewScript = $derived(
		versionState.kind === 'preview' ? versionState.script : null
	);
	let loadedContent = $derived(previewScript?.content ?? scriptLoader.data?.content ?? '');
	let loadedLanguage = $derived(
		((previewScript?.language ?? scriptLoader.data?.language) as ScriptLang | undefined) ?? 'python3'
	);
	let loadedRuntime = $derived(
		previewScript?.runtime ?? scriptLoader.data?.runtime ?? DEFAULT_PYTHON_RUNTIME
	);
	let loadedRequirements = $derived(
		previewScript?.requirements ?? scriptLoader.data?.requirements ?? []
	);
	// Re-keys derived state when the underlying version changes so editor state resets.
	let loadedScriptKey = $derived(
		previewScript?.hash ?? scriptLoader.data?.hash ?? scriptPath
	);

	// Writable derived values reset from the loaded script, then diverge while the editor mutates them.
	let content = $derived(loadedContent);
	let summary = $derived.by(() => {
		void loadedScriptKey;
		return '';
	});
	let language = $derived(loadedLanguage);
	let runtime = $derived(loadedRuntime);
	let requirements = $derived([...loadedRequirements]);
	let saving = $state(false);
	let draftStorageVersion = $state(0);
	// Baseline values are also writable so a successful save can reset dirty detection immediately.
	let baseContent = $derived(loadedContent);
	let baseLanguage = $derived(loadedLanguage);
	let baseRuntime = $derived(loadedRuntime);
	let baseRequirements = $derived([...loadedRequirements]);
	let currentRuntime = $derived(language === 'python3' ? runtime : DEFAULT_PYTHON_RUNTIME);
	let baseEffectiveRuntime = $derived(
		baseLanguage === 'python3' ? baseRuntime : DEFAULT_PYTHON_RUNTIME
	);
	let currentRequirements = $derived(language === 'python3' ? requirements : []);
	let baseEffectiveRequirements = $derived(baseLanguage === 'python3' ? baseRequirements : []);
	let draftWorkspaceId = $derived(scriptLoader.data?.workspace_id ?? workspace.id);
	let currentDraftBaseline = $derived({
		content: loadedContent,
		language: loadedLanguage,
		runtime: loadedLanguage === 'python3' ? loadedRuntime : DEFAULT_PYTHON_RUNTIME,
		requirements: loadedLanguage === 'python3' ? loadedRequirements : []
	});

	let isDirty = $derived(
		content !== baseContent ||
		language !== baseLanguage ||
		currentRuntime !== baseEffectiveRuntime ||
		currentRequirements.join(',') !== baseEffectiveRequirements.join(',')
	);

	let canSave = $derived(
		!isReadonly &&
			isDirty &&
			!scriptLoader.loading &&
			!scriptLoader.error &&
			!!scriptLoader.data &&
			!saving
	);
	let saveDisabledReason = $derived.by(() => {
		if (isReadonly) return 'Switch to current to edit';
		if (saving) return 'Saving...';
		if (!isDirty) return 'No changes to save';
		if (scriptLoader.loading) return 'Loading script';
		if (scriptLoader.error) return 'Resolve the load error before saving';
		if (!scriptLoader.data) return 'Script is not loaded';
		return undefined;
	});

	let availableDraft: ScriptDraft | null = $derived.by(() => {
		void draftStorageVersion;
		if (isDirty || !scriptLoader.data || !scriptPath) return null;

		const draft = readScriptDraft(draftWorkspaceId, scriptPath);
		if (!draft || !draftDiffersFromBaseline(draft, currentDraftBaseline)) return null;
		return draft;
	});

	// -- Run state -------------------------------------------------------------

	let activeRunId = $state<string | null>(null);
	let running = $state(false);
	let runOptionsOpen = $state(false);
	let gearAnchor = $state<HTMLDivElement | undefined>();

	type ResultPanel =
		| { visible: false }
		| { visible: true; success: boolean; result: unknown };
	let resultPanel = $state<ResultPanel>({ visible: false });
	// Independent of `visible`: once a result arrives, the header stays in the
	// DOM and the user can collapse/expand the JSON body without losing it.
	let resultExpanded = $state(true);

	// -- Right-panel tab state ------------------------------------------------

	type RightTab = 'logs' | 'history';
	let activeTab = $state<RightTab>('logs');

	// View / Restore modal state — both pre-fetched ScriptResponse objects
	// rather than just hashes so the modal renders synchronously once open.
	let viewModalScript = $state<ScriptResponse | null>(null);
	let restoreCandidate = $state<ScriptResponse | null>(null);
	let restoreModalOpen = $state(false);

	function handleViewVersion(script: ScriptResponse): void {
		viewModalScript = script;
	}

	function applyRestore(script: ScriptResponse): void {
		content = script.content;
		language = (script.language as ScriptLang) ?? 'python3';
		runtime = script.runtime ?? DEFAULT_PYTHON_RUNTIME;
		requirements = [...script.requirements];
		toastInfo(`Restored content from ${script.hash.slice(0, 8)}. Save to create a new version.`);
	}

	function handleRestoreVersion(script: ScriptResponse): void {
		// Always close the diff modal first if it's open — the user has
		// chosen "I want this version", so the comparison view is done.
		viewModalScript = null;
		if (isDirty) {
			restoreCandidate = script;
			restoreModalOpen = true;
		} else {
			applyRestore(script);
		}
	}

	function confirmRestore(): void {
		if (restoreCandidate) applyRestore(restoreCandidate);
		restoreModalOpen = false;
		restoreCandidate = null;
	}

	function cancelRestore(): void {
		restoreModalOpen = false;
		restoreCandidate = null;
	}

	// ARIA tablist standard pattern: arrow keys move focus + selection.
	// Home/End jump to the first/last tab. Skips wrapping (only two tabs).
	function onTabKeydown(e: KeyboardEvent): void {
		const tabs: RightTab[] = ['logs', 'history'];
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
			activeTab = tabs[nextIndex];
			document.getElementById(`tab-${activeTab}`)?.focus();
		}
	}

	function persistCurrentDraft(): boolean {
		if (!isDirty || !scriptPath || !scriptLoader.data) return false;
		return writeScriptDraft({
			version: 1,
			workspaceId: draftWorkspaceId,
			scriptPath,
			baseHash: loadedScriptKey,
			content,
			language,
			runtime: currentRuntime,
			requirements: [...currentRequirements],
			summary,
			updatedAt: new Date().toISOString()
		});
	}

	function clearCurrentDraft() {
		if (!scriptPath) return;
		removeScriptDraft(draftWorkspaceId, scriptPath);
		draftStorageVersion += 1;
	}

	function restoreDraft() {
		if (!availableDraft) return;
		content = availableDraft.content;
		language = availableDraft.language;
		runtime = availableDraft.runtime;
		requirements = [...availableDraft.requirements];
		summary = availableDraft.summary;
		toastInfo('Restored local draft');
	}

	function discardDraft() {
		clearCurrentDraft();
		toastInfo('Discarded local draft');
	}

	async function confirmSave(): Promise<boolean> {
		if (!canSave || !scriptPath || !scriptLoader.data) return false;
		const wsId = workspace.id;
		persistCurrentDraft();
		saving = true;
		try {
			const response = await api.forWorkspace(wsId).createScript({
				path: scriptPath,
				// Preserve the script's display name across version saves.
				name: scriptLoader.data?.name || scriptPath,
				language,
				content,
				summary: summary || undefined,
				requirements: currentRequirements.length > 0 ? currentRequirements : undefined,
				runtime: language === 'python3' ? runtime : undefined
			});
			if (workspace.id !== wsId) return false;
			toastSuccess(`Saved ${response.hash.slice(0, 12)}`);
			summary = '';
			clearCurrentDraft();
			baseContent = content;
			baseLanguage = language;
			baseRuntime = currentRuntime;
			baseRequirements = [...currentRequirements];
			scriptLoader.reload();
			return true;
		} catch (e) {
			if (workspace.id !== wsId) return false;
			if (e instanceof ApiClientError) {
				toastError(`${e.status}: ${e.body || e.message}`);
			} else {
				toastError(e instanceof Error ? e.message : 'Save failed');
			}
			return false;
		} finally {
			saving = false;
		}
	}

	// `running` spans the whole run lifecycle, not just the createRun POST:
	// the button stays disabled (and spinning) from submit until the run reaches
	// a terminal result via the LogViewer (handleResult) — otherwise the POST
	// returns in milliseconds and the user could fire off a second run while the
	// first is still queued/executing. Cleared on: submit failure, terminal
	// result, stream error, and workspace switch.
	async function handleRun(): Promise<void> {
		if (!scriptPath || !scriptLoader.data || running) return;
		const wsId = workspace.id;
		running = true;
		try {
			const response = await api.forWorkspace(wsId).createRun({
				kind: 'preview',
				raw_code: content,
				language,
				requirements:
					language === 'python3' && currentRequirements.length > 0
						? currentRequirements
						: undefined,
				custom_image: language === 'python3' ? currentRuntime : undefined,
				args: runOptions.args,
				tag: runOptions.tag,
				timeout: runOptions.timeout,
				priority: runOptions.priority,
				cpus: runOptions.cpus,
				memory_mb: runOptions.memoryMb,
				disk_mb: runOptions.diskMb,
				team_owner: runOptions.teamOwner ?? undefined
			});
			if (workspace.id !== wsId) return;
			resultPanel = { visible: false };
			activeRunId = response.id;
			// Leave `running` true: the run is now executing and observed by the
			// LogViewer; handleResult / handleLogError releases the button.
		} catch (e) {
			if (workspace.id !== wsId) return;
			// Submit failed — no run started, so re-enable Run immediately.
			running = false;
			if (e instanceof ApiClientError) {
				toastError(`${e.status}: ${e.body || e.message}`);
			} else {
				toastError(e instanceof Error ? e.message : 'Run failed');
			}
		}
	}

	function handleResult(event: RunResultEvent): void {
		resultPanel = { visible: true, success: event.success, result: event.result };
		resultExpanded = true;
		// Run reached a terminal result — allow another run.
		running = false;
	}

	function handleLogError(message: string): void {
		// Stream error: stop treating the run as in-flight so the user isn't locked
		// out of re-running. The run may still be executing server-side, but a
		// parallel preview re-run is acceptable (same trade-off as a workspace switch).
		running = false;
		toastError(`Log stream error: ${message}`);
	}

	$effect(() => {
		if (!scriptLoader.data || !scriptPath) return;
		// Only manage the HEAD draft in editable mode. In preview, the baseline
		// reflects the historical version, so this could spuriously decide the
		// stored HEAD draft is redundant and delete the user's unsaved work.
		if (viewMode !== 'editable') return;

		if (isDirty) {
			persistCurrentDraft();
			return;
		}

		if (!availableDraft) {
			removeScriptDraft(draftWorkspaceId, scriptPath);
		}
	});

	// Workspace switch: scrap any in-flight run state.
	//
	// activeRunId belongs to the previous workspace — re-streaming it against
	// the new workspace would 404 (best case) or leak run data across workspaces
	// (worst case if IDs collide). The result panel similarly references the
	// previous workspace's run, so we hide it too.
	//
	// `running` is also reset: any handleRun() fetch still in flight from the
	// previous workspace will be discarded by its own `if (workspace.id !== wsId)`
	// guard, so we never want the new workspace's UI to show a phantom "running"
	// state owed to a request that will never affect it. Minor known caveat:
	// during the brief window between switch and old-fetch-resolves, the user
	// could click Run on the new workspace and trigger a parallel preview run.
	// Acceptable for preview kind.
	$effect(() => {
		void workspace.id;
		activeRunId = null;
		resultPanel = { visible: false };
		running = false;
	});

	// If the sticky teamOwner is no longer a member of the current workspace's
	// team list, clear it so createRun does not send a stale team_owner.
	$effect(() => {
		if (teamsLoader.loading || teamsLoader.error) return;
		const items = teamsLoader.data?.items;
		if (!items) return;
		const owner = runOptions.teamOwner;
		if (owner && !items.some((t) => t.name === owner)) {
			runOptions.teamOwner = null;
		}
	});

	beforeNavigate((navigation) => {
		if (!isDirty) return;

		persistCurrentDraft();

		if (navigation.to?.url.pathname === '/user/login') return;

		const message =
			'You have unsaved changes. A local draft has been saved. Leave this page?';

		// Full-page unload: the draft is already persisted above; confirmNavigation
		// no-ops on willUnload, so nothing to cancel here.
		confirmNavigation(navigation, {
			title: 'Leave this page?',
			message,
			confirmLabel: 'Leave',
			variant: 'danger'
		});
	});

</script>

<svelte:head>
	<title>{scriptPath || 'Edit Script'} | CoveFlow</title>
</svelte:head>

{#snippet runOptionsControl()}
	<div bind:this={gearAnchor} class="relative">
		<Button
			variant="ghost"
			size="sm"
			aria-expanded={runOptionsOpen}
			onclick={() => (runOptionsOpen = !runOptionsOpen)}
		>
			Run options
			<ChevronDown
				size={12}
				class="transition-transform duration-200 {runOptionsOpen ? 'rotate-180' : ''}"
			/>
		</Button>
		<RunOptionsPopover
			open={runOptionsOpen}
			teams={teamsLoader.data?.items ?? []}
			teamsLoading={teamsLoader.loading}
			teamsError={!!teamsLoader.error}
			anchor={gearAnchor}
			onClose={() => (runOptionsOpen = false)}
		/>
	</div>
{/snippet}

{#snippet rightPanel()}
	<!--
		Tabs let the user switch between live logs and version history without
		unmounting either panel — LogViewer keeps polling in the background so
		coming back to it shows the most recent content instantly.
		-mb-px + matching border-b-2 on both tabs avoids the active-tab indicator
		double-stacking on top of the parent bar's border, and keeps both tabs the
		same height so the row doesn't jump on switch.
	-->
	<div
		role="tablist"
		aria-label="Right panel sections"
		class="flex border-b border-border bg-surface-raised text-sm"
	>
		<button
			type="button"
			id="tab-logs"
			role="tab"
			aria-selected={activeTab === 'logs'}
			aria-controls="panel-logs"
			tabindex={activeTab === 'logs' ? 0 : -1}
			class="-mb-px inline-flex items-center gap-1.5 border-b-2 px-4 py-1.5 {activeTab === 'logs'
				? 'border-info text-text'
				: 'border-transparent text-text-tertiary hover:text-text'}"
			onclick={() => (activeTab = 'logs')}
			onkeydown={onTabKeydown}
		>
			<Activity size={12} />
			Logs
		</button>
		<button
			type="button"
			id="tab-history"
			role="tab"
			aria-selected={activeTab === 'history'}
			aria-controls="panel-history"
			tabindex={activeTab === 'history' ? 0 : -1}
			class="-mb-px inline-flex items-center gap-1.5 border-b-2 px-4 py-1.5 {activeTab === 'history'
				? 'border-info text-text'
				: 'border-transparent text-text-tertiary hover:text-text'}"
			onclick={() => (activeTab = 'history')}
			onkeydown={onTabKeydown}
		>
			<History size={12} />
			History
		</button>
	</div>

	<div
		id="panel-logs"
		role="tabpanel"
		aria-labelledby="tab-logs"
		class="flex min-h-0 flex-1 flex-col {activeTab === 'logs' ? '' : 'hidden'}"
	>
		<LogViewer
			workspaceId={workspace.id}
			runId={activeRunId}
			onResult={handleResult}
			onError={handleLogError}
			class="min-h-0 flex-1"
		/>
		{#if resultPanel.visible}
			<RunResultPanel
				success={resultPanel.success}
				result={resultPanel.result}
				expanded={resultExpanded}
				onToggle={() => (resultExpanded = !resultExpanded)}
			/>
		{/if}
	</div>

	<div
		id="panel-history"
		role="tabpanel"
		aria-labelledby="tab-history"
		class="flex min-h-0 flex-1 flex-col {activeTab === 'history' ? '' : 'hidden'}"
	>
		<ScriptHistory
			path={scriptPath}
			currentHash={scriptLoader.data?.hash ?? ''}
			onView={handleViewVersion}
			onRestore={handleRestoreVersion}
			class="min-h-0 flex-1"
		/>
	</div>
{/snippet}

<ScriptEditShell
	{scriptPath}
	bind:summary
	{isDirty}
	{canSave}
	{saving}
	{saveDisabledReason}
	onBack={() => goto('/scripts')}
	onSave={confirmSave}
	onRun={handleRun}
	{running}
	{runOptionsControl}
>
	<ScriptEditContent
		loading={scriptLoader.loading}
		error={scriptLoader.error}
		loaded={!!scriptLoader.data}
		right={rightPanel}
	>
		<!--
			Version banners (Q6 / Q7): sit immediately above the editor so the
			user sees the verdict before reading code, and so the dismiss/exit
			actions are inside the same focus region.
		-->
		{#if viewMode === 'preview' && previewScript}
			{@const headHash = scriptLoader.data?.hash}
			<div
				role="status"
				aria-label="Viewing historical version"
				class="flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-border bg-info/10 px-3 py-1.5 text-xs text-text"
			>
				<span class="font-medium">Viewing v{previewScript.hash.slice(0, 7)}</span>
				{#if headHash}
					<span class="text-text-tertiary">·</span>
					<span class="text-text-secondary">Current is v{headHash.slice(0, 7)}</span>
				{/if}
				<div class="ml-auto">
					<Button variant="secondary" size="sm" onclick={exitPreview}>
						Switch to current to edit
					</Button>
				</div>
			</div>
		{:else if viewMode === 'preview-loading'}
			<div class="border-b border-border bg-surface-alt/40 px-3 py-1.5 text-xs text-text-tertiary">
				Loading version v{versionParam?.slice(0, 7)}…
			</div>
		{:else if versionState.kind === 'fallback' && !versionBannerDismissed}
			<div
				role="alert"
				class="flex items-center gap-2 border-b border-border bg-warning/15 px-3 py-1.5 text-xs text-text"
			>
				<span>
					⚠ Version <span class="font-mono">v{versionParam?.slice(0, 7)}</span> no longer available, showing current.
				</span>
				<button
					type="button"
					class="ml-auto rounded px-1.5 py-0.5 text-text-secondary hover:bg-surface-raised hover:text-text"
					onclick={() => (versionBannerDismissed = true)}
					aria-label="Dismiss notice"
				>
					Dismiss
				</button>
			</div>
		{:else if versionState.kind === 'error' && !versionBannerDismissed}
			<div
				role="alert"
				class="flex items-center gap-2 border-b border-border bg-warning/15 px-3 py-1.5 text-xs text-text"
			>
				<span>
					⚠ Couldn't load version <span class="font-mono">v{versionParam?.slice(0, 7)}</span> — showing current.
				</span>
				<button
					type="button"
					class="ml-auto rounded px-1.5 py-0.5 text-text-secondary hover:bg-surface-raised hover:text-text"
					onclick={retryVersionLoad}
				>
					Retry
				</button>
				<button
					type="button"
					class="rounded px-1.5 py-0.5 text-text-secondary hover:bg-surface-raised hover:text-text"
					onclick={() => (versionBannerDismissed = true)}
					aria-label="Dismiss notice"
				>
					Dismiss
				</button>
			</div>
		{/if}
		{#if viewMode === 'editable'}
			<!-- Draft restore only applies to the editable HEAD; in preview the
			     Restore would write into readonly state and be dropped. -->
			<ScriptDraftNotice
				draft={availableDraft}
				isStale={!!availableDraft && availableDraft.baseHash !== loadedScriptKey}
				onRestore={restoreDraft}
				onDiscard={discardDraft}
			/>
		{/if}
		<ScriptEditor
			bind:content
			bind:language
			bind:runtime
			bind:requirements
			readonly={isReadonly}
			showRuntime
		/>
	</ScriptEditContent>
</ScriptEditShell>

<VersionDiffModal
	open={viewModalScript !== null}
	oldScript={viewModalScript}
	currentContent={content}
	currentLanguage={language}
	onClose={() => (viewModalScript = null)}
	onRestore={handleRestoreVersion}
/>

{#if restoreCandidate}
	<RestoreConfirmModal
		bind:open={restoreModalOpen}
		versionHash={restoreCandidate.hash}
		onConfirm={confirmRestore}
		onCancel={cancelRestore}
	/>
{/if}
