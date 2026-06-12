<script lang="ts">
	import { beforeNavigate, goto } from '$app/navigation';
	import { page } from '$app/state';
	import { ArrowLeft, ChevronDown, FilePlus } from '@lucide/svelte';
	import { onMount, tick } from 'svelte';

	import ScriptEditContent from '$lib/components/scripts/ScriptEditContent.svelte';
	import ScriptEditor from '$lib/components/scripts/ScriptEditor.svelte';
	import { DEFAULT_PYTHON_RUNTIME } from '$lib/components/scripts/editor/languages';
	import LogViewer from '$lib/components/runs/LogViewer.svelte';
	import RunResultPanel from '$lib/components/runs/RunResultPanel.svelte';
	import RunOptionsPopover from '$lib/components/runs/RunOptionsPopover.svelte';
	import { runOptions } from '$lib/components/runs/run-options/store.svelte';
	import Button from '$lib/components/common/Button.svelte';
	import IconButton from '$lib/components/common/IconButton.svelte';
	import TextInput from '$lib/components/common/TextInput.svelte';
	import { api, ApiClientError } from '$lib/services/api';
	import { useWorkspaceLoader } from '$lib/services/workspace-loader.svelte';
	import { encodePath } from '$lib/services/url';
	import { confirmNavigation } from '$lib/stores/confirm.svelte';
	import { workspace } from '$lib/stores/workspace.svelte';
	import { toastError, toastSuccess } from '$lib/toast';
	import type { RunResultEvent, ScriptLang } from '$lib/types';

	// -- Form state -----------------------------------------------------------

	const DEFAULT_CONTENT = 'print("hello from CoveFlow")\n';
	const PREFILL_KEY = 'script-prefill';
	// Languages accepted from a prefill payload. Keep in sync with ScriptLang;
	// membership-checking (vs hardcoding 'python3') lets future languages prefill.
	const ALLOWED_SCRIPT_LANGS: readonly ScriptLang[] = ['python3'];

	// Path is built from an optional folder + the name (name is the path's leaf),
	// so a new script's path always ends with its name. Path is the immutable id
	// after creation; the display name stays separately editable later.
	// Prefilled from ?folder when "new script" is triggered from a tree folder.
	let folder = $state(page.url.searchParams.get('folder') ?? '');
	let name = $state('');
	// SvelteKit reuses this component across /scripts/add?folder=… navigations, so
	// re-sync the folder when the query changes (otherwise it keeps the old value).
	$effect(() => {
		// Sync to the query on every navigation — including the param-less "New"
		// (goto('/scripts/add')), which should reset to a blank folder.
		folder = page.url.searchParams.get('folder') ?? '';
	});
	const computedPath = $derived.by(() => {
		const n = name.trim();
		const f = folder.trim().replace(/^\/+|\/+$/g, '');
		return f ? `${f}/${n}` : n;
	});
	let summary = $state('');
	let content = $state(DEFAULT_CONTENT);
	// Baseline the dirty-check compares against: DEFAULT_CONTENT normally, or the
	// prefilled content after a "Copy to script", so merely-copied (unedited)
	// content doesn't trigger the discard prompt.
	let baselineContent = $state(DEFAULT_CONTENT);
	let language = $state<ScriptLang>('python3');
	let runtime = $state(DEFAULT_PYTHON_RUNTIME);
	let requirements = $state<string[]>([]);
	let saving = $state(false);
	let prefilledFromRun = $state(false);

	// One-shot prefill consumed from sessionStorage. Set by RunCodePanel's
	// "Copy to script" action on the run page. Cleared immediately so a refresh
	// of this page (or arriving fresh) does not silently reuse stale content.
	onMount(() => {
		try {
			const raw = sessionStorage.getItem(PREFILL_KEY);
			if (!raw) return;
			sessionStorage.removeItem(PREFILL_KEY);
			const parsed = JSON.parse(raw) as {
				content?: unknown;
				language?: unknown;
				requirements?: unknown;
			};
			if (typeof parsed.content === 'string' && parsed.content.length > 0) {
				content = parsed.content;
				baselineContent = parsed.content;
				prefilledFromRun = true;
			}
			if (
				typeof parsed.language === 'string' &&
				(ALLOWED_SCRIPT_LANGS as readonly string[]).includes(parsed.language)
			) {
				language = parsed.language as ScriptLang;
			}
			if (Array.isArray(parsed.requirements)) {
				requirements = parsed.requirements.filter((r): r is string => typeof r === 'string');
			}
		} catch {
			// Malformed payload — ignore. The default content stays.
		}
	});

	// -- Run state ------------------------------------------------------------

	let activeRunId = $state<string | null>(null);
	let running = $state(false);
	let runOptionsOpen = $state(false);
	let gearAnchor = $state<HTMLDivElement | undefined>();

	type ResultPanel =
		| { visible: false }
		| { visible: true; success: boolean; result: unknown };
	let resultPanel = $state<ResultPanel>({ visible: false });
	let resultExpanded = $state(true);

	const teamsLoader = useWorkspaceLoader((ws) => ws.listTeams());

	let currentRequirements = $derived(language === 'python3' ? requirements : []);
	let currentRuntime = $derived(language === 'python3' ? runtime : DEFAULT_PYTHON_RUNTIME);

	let canSave = $derived(
		name.trim() !== '' && !name.includes('/') && content.trim() !== '' && !saving
	);
	let saveDisabledReason = $derived.by(() => {
		if (saving) return 'Saving...';
		if (name.trim() === '') return 'Enter a name';
		if (name.includes('/')) return 'Name cannot contain "/" (use the folder field)';
		if (content.trim() === '') return 'Editor is empty';
		return undefined;
	});

	// -- Save popover (summary capture, mirrors ScriptEditShell behaviour) -----

	const SAVE_SUMMARY_ID = 'add-save-summary';
	let savePopoverOpen = $state(false);
	let popoverElement: HTMLDivElement | undefined = $state();

	async function openSavePopover() {
		if (!canSave) return;
		savePopoverOpen = true;
		await tick();
		document.getElementById(SAVE_SUMMARY_ID)?.focus();
	}

	function handleSummaryKeydown(e: KeyboardEvent) {
		if (e.key === 'Enter') {
			e.preventDefault();
			void submitSave();
		}
	}

	async function submitSave(): Promise<void> {
		if (!canSave) return;
		const wsId = workspace.id;
		saving = true;
		try {
			const response = await api.forWorkspace(wsId).createScript({
				path: computedPath,
				name: name.trim(),
				language,
				content,
				summary: summary || undefined,
				requirements: currentRequirements.length > 0 ? currentRequirements : undefined,
				runtime: language === 'python3' ? runtime : undefined
			});
			if (workspace.id !== wsId) return;
			toastSuccess(`Saved ${response.hash.slice(0, 12)}`);
			// Jump to the edit page for the freshly created script — the user
			// can now use Run History and continue editing in the standard flow.
			savePopoverOpen = false;
			await goto(`/scripts/edit/${encodePath(computedPath)}`);
		} catch (e) {
			if (workspace.id !== wsId) return;
			if (e instanceof ApiClientError) {
				toastError(`${e.status}: ${e.body || e.message}`);
			} else {
				toastError(e instanceof Error ? e.message : 'Save failed');
			}
		} finally {
			saving = false;
		}
	}

	// -- Run handler (preview kind, no saved script needed) -------------------

	async function handleRun(): Promise<void> {
		if (running) return;
		if (content.trim() === '') {
			toastError('Editor is empty');
			return;
		}
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
		} catch (e) {
			if (workspace.id !== wsId) return;
			if (e instanceof ApiClientError) {
				toastError(`${e.status}: ${e.body || e.message}`);
			} else {
				toastError(e instanceof Error ? e.message : 'Run failed');
			}
		} finally {
			if (workspace.id === wsId) running = false;
		}
	}

	function handleResult(event: RunResultEvent): void {
		resultPanel = { visible: true, success: event.success, result: event.result };
		resultExpanded = true;
	}

	function handleLogError(message: string): void {
		toastError(`Log stream error: ${message}`);
	}

	// Workspace switch: scrap any in-flight run state (same reasoning as edit page).
	$effect(() => {
		void workspace.id;
		activeRunId = null;
		resultPanel = { visible: false };
		running = false;
	});

	// Guard against accidental navigation losing the new script content.
	// Skip when: save is in flight, nothing's been touched, or we're navigating
	// to the freshly-saved script's edit page (legitimate post-save jump).
	// Has the user entered anything worth warning about before they leave?
	const hasUnsavedNew = $derived(
		!saving &&
			(folder.trim() !== '' || name.trim() !== '' || content !== baselineContent || summary !== '')
	);

	beforeNavigate((navigation) => {
		if (!hasUnsavedNew) return;
		if (navigation.to?.url.pathname?.startsWith('/scripts/edit/')) return;
		// Full-page unload (reload / tab close) is guarded by the native
		// onbeforeunload below; confirmNavigation no-ops on it. In-app nav uses the modal.
		confirmNavigation(navigation, {
			title: 'Discard new script?',
			message: 'Your unsaved script will be lost.',
			confirmLabel: 'Discard',
			variant: 'danger'
		});
	});

	function onBeforeUnload(e: BeforeUnloadEvent): void {
		if (hasUnsavedNew) {
			e.preventDefault();
			e.returnValue = '';
		}
	}

	// Dismiss save popover on outside click / Escape.
	$effect(() => {
		if (!savePopoverOpen) return;

		function handlePointerDown(event: PointerEvent) {
			const target = event.target;
			if (target instanceof Node && popoverElement?.contains(target)) return;
			savePopoverOpen = false;
		}

		function handleDocumentKeydown(event: KeyboardEvent) {
			if (event.key === 'Escape') savePopoverOpen = false;
		}

		document.addEventListener('pointerdown', handlePointerDown);
		document.addEventListener('keydown', handleDocumentKeydown);
		return () => {
			document.removeEventListener('pointerdown', handlePointerDown);
			document.removeEventListener('keydown', handleDocumentKeydown);
		};
	});
</script>

<svelte:window onbeforeunload={onBeforeUnload} />

<svelte:head>
	<title>New script | CoveFlow</title>
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
{/snippet}

<div class="flex h-svh flex-col lg:h-svh max-lg:h-[calc(100svh-48px)]">
	<div class="flex items-center gap-2 border-b border-border px-3 py-2">
		<IconButton onclick={() => goto('/scripts')} aria-label="Back to scripts">
			<ArrowLeft size={16} />
		</IconButton>

		<div class="flex items-center gap-1.5 text-sm">
			<FilePlus size={14} class="text-text-tertiary" />
			<span class="font-medium text-text">New script</span>
			{#if prefilledFromRun && content === baselineContent}
				<!-- Hide once the user edits away from the copied content. -->
				<span
					class="ml-1 rounded bg-info/15 px-1.5 py-0.5 text-xs text-info"
					title="Content was copied from a previous preview run"
				>
					copied from run
				</span>
			{/if}
		</div>

		<!-- Folder (optional) + name → path = folder/name. Name is the path's leaf
			 and can't contain "/". Path is shown read-only (derived); it becomes the
			 immutable id once saved. -->
		<div class="ml-2 w-40">
			<TextInput id="add-folder" label="" aria-label="Folder" bind:value={folder} placeholder="folder (optional)" mono />
		</div>
		<div class="w-40">
			<TextInput id="add-name" label="" aria-label="Script name" bind:value={name} placeholder="name (required)" />
		</div>
		<div class="flex min-w-0 items-center gap-1 text-xs text-text-tertiary" title="Resulting path (the script's id)">
			<span class="shrink-0">path:</span>
			<span class="truncate font-mono text-text-secondary">{computedPath || '—'}</span>
		</div>

		<div class="flex-1"></div>

		{@render runOptionsControl()}

		<Button
			variant="ghost"
			size="sm"
			onclick={() => void handleRun()}
			loading={running}
			disabled={running || content.trim() === ''}
		>
			Run preview
		</Button>

		<div class="relative">
			<Button
				variant="primary"
				size="sm"
				onclick={openSavePopover}
				loading={saving}
				disabled={!canSave}
				title={saveDisabledReason}
			>
				Save
			</Button>

			{#if savePopoverOpen}
				<div
					bind:this={popoverElement}
					class="absolute right-0 top-full z-50 mt-2 w-80 rounded-lg border border-border bg-surface-raised p-3 shadow-lg"
				>
					<TextInput
						id={SAVE_SUMMARY_ID}
						label="Summary"
						bind:value={summary}
						placeholder="Initial version"
						onkeydown={handleSummaryKeydown}
					/>
					<div class="mt-2 flex justify-end gap-2">
						<Button size="sm" variant="ghost" onclick={() => (savePopoverOpen = false)}>
							Cancel
						</Button>
						<Button
							size="sm"
							variant="primary"
							onclick={() => void submitSave()}
							loading={saving}
							disabled={!canSave}
						>
							Confirm
						</Button>
					</div>
				</div>
			{/if}
		</div>
	</div>

	<ScriptEditContent loading={false} error="" loaded={true} right={rightPanel}>
		<ScriptEditor bind:content bind:language bind:runtime bind:requirements showRuntime />
	</ScriptEditContent>
</div>
