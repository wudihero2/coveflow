<script lang="ts">
	import { onMount, untrack } from 'svelte';
	import { goto } from '$app/navigation';
	import { ExternalLink, FlaskConical, FileCode, Save, TriangleAlert } from '@lucide/svelte';

	import ScriptEditor from '$lib/components/scripts/ScriptEditor.svelte';
	import Button from '$lib/components/common/Button.svelte';
	import { api, ApiClientError } from '$lib/services/api';
	import { encodePath } from '$lib/services/url';
	import { toastError } from '$lib/toast';
	import type { RunResponse, ScriptLang } from '$lib/types';

	interface Props {
		run: RunResponse;
		workspaceId: string;
	}

	let { run, workspaceId }: Props = $props();

	// Path existence check: lifted-state pattern so a re-run navigation
	// (run.id changes) refetches without leaking the previous run's verdict.
	type PathState =
		| { kind: 'preview' } // No saved script (raw_code only)
		| { kind: 'loading' }
		| { kind: 'exists'; currentHash: string }
		| { kind: 'deleted' }
		| { kind: 'unknown' }; // Fetch failed for non-404 reasons — treat as exists to not falsely accuse

	let pathState = $state<PathState>({ kind: 'loading' });
	// Dedupe key includes workspace + path, not just run.id: switching workspace
	// (component stays mounted) or a record gaining/losing a script_path must
	// re-check, otherwise the verdict (and Edit link) goes stale.
	let lastCheckedKey: string | null = null;
	// Epoch guards the async resolution: only the latest in-flight fetch may write
	// pathState, so a slower earlier fetch can't clobber a newer result.
	let fetchEpoch = 0;

	$effect(() => {
		const runId = run.id;
		const path = run.script_path;
		const wsId = workspaceId;
		const key = `${wsId}::${runId}::${path ?? ''}`;
		if (lastCheckedKey === key) return;
		lastCheckedKey = key;

		untrack(() => {
			if (!path) {
				pathState = { kind: 'preview' };
				return;
			}
			const epoch = ++fetchEpoch;
			pathState = { kind: 'loading' };
			void (async () => {
				try {
					const head = await api.forWorkspace(wsId).getScriptByPath(path);
					if (epoch === fetchEpoch) pathState = { kind: 'exists', currentHash: head.hash };
				} catch (e) {
					if (epoch !== fetchEpoch) return;
					if (e instanceof ApiClientError && e.status === 404) {
						pathState = { kind: 'deleted' };
					} else {
						pathState = { kind: 'unknown' };
					}
				}
			})();
		});
	});

	// Flow child runs (and any script-by-hash run) carry no inline raw_code — the
	// worker loads the body from the script table at execution. Fetch that exact
	// version by hash so the Code tab isn't blank.
	let hashContent = $state<string | null>(null);
	let hashLang = $state<ScriptLang | null>(null);
	let lastContentKey: string | null = null;
	let contentEpoch = 0;
	$effect(() => {
		const runId = run.id;
		const hash = run.script_hash;
		const hasRaw = !!run.raw_code;
		const wsId = workspaceId;
		const key = `${wsId}::${runId}`;
		if (lastContentKey === key) return;
		lastContentKey = key;
		hashContent = null;
		hashLang = null;
		if (hasRaw || !hash) return;
		const epoch = ++contentEpoch;
		void (async () => {
			try {
				const s = await api.forWorkspace(wsId).getScriptByHash(hash);
				if (epoch !== contentEpoch) return;
				hashContent = s.content;
				hashLang = s.language as ScriptLang;
			} catch {
				/* leave null → "No code recorded" */
			}
		})();
	});

	let codeContent = $derived(run.raw_code ?? hashContent);
	let editorLang = $derived<ScriptLang>(
		(run.language as ScriptLang | null) ?? hashLang ?? 'python3'
	);

	let editUrl = $derived.by(() => {
		// 'unknown' = existence couldn't be verified (non-404 error); per the type's
		// "treat as exists" intent, still offer Edit rather than a dead action row.
		if (!run.script_path || (pathState.kind !== 'exists' && pathState.kind !== 'unknown'))
			return null;
		const encoded = encodePath(run.script_path);
		// Q8: when ?version equals current HEAD, the editor strips it and goes
		// editable; passing the param uniformly keeps this side simple.
		const hash = run.script_hash;
		return hash
			? `/scripts/edit/${encoded}?version=${encodeURIComponent(hash)}`
			: `/scripts/edit/${encoded}`;
	});

	let staleHint = $derived.by(() => {
		if (pathState.kind !== 'exists' || !run.script_hash) return null;
		if (pathState.currentHash === run.script_hash) return null;
		return pathState.currentHash;
	});

	function copyToNewScript(): void {
		if (!run.raw_code) return;
		// Q9: sessionStorage is one-shot; the /scripts/add page consumes and clears.
		// setItem throws QuotaExceededError synchronously for large raw_code, which
		// would otherwise abort before navigating and look like the button did
		// nothing — guard and surface it instead.
		try {
			sessionStorage.setItem(
				'script-prefill',
				JSON.stringify({
					content: run.raw_code,
					language: run.language ?? 'python3',
					requirements: run.requirements ?? [],
					sourceRunId: run.id
				})
			);
		} catch {
			toastError('This run is too large to copy into a new script.');
			return;
		}
		void goto('/scripts/add');
	}
</script>

<div class="flex min-h-0 flex-1 flex-col">
	<!--
		Header: identifies which script + version this code came from. The diff
		hint ("current: ...") tells the user when the latest version differs from
		what actually ran, so they don't false-attribute behaviour to stale code.
	-->
	<div
		class="flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-border bg-surface-alt/40 px-3 py-1.5 text-xs"
	>
		{#if pathState.kind === 'preview'}
			<div class="flex items-center gap-1.5 text-text-secondary">
				<FlaskConical size={12} class="shrink-0 text-text-tertiary" aria-hidden="true" />
				<span>Preview run · not saved</span>
			</div>
			<div class="ml-auto">
				{#if run.raw_code}
					<Button variant="ghost" size="sm" onclick={copyToNewScript}>
						<Save size={12} aria-hidden="true" />
						Copy to script
					</Button>
				{/if}
			</div>
		{:else}
			<div class="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-0.5 text-text-secondary">
				<FileCode size={12} class="shrink-0 text-text-tertiary" aria-hidden="true" />
				<span class="truncate font-mono text-text">{run.script_path}</span>
				{#if run.script_hash}
					<span class="text-text-tertiary">·</span>
					<span class="font-mono" title={run.script_hash}>
						v{run.script_hash.slice(0, 7)}
					</span>
				{/if}
				{#if staleHint}
					<span class="text-text-tertiary">·</span>
					<span class="font-mono text-text-tertiary" title={staleHint}>
						current: v{staleHint.slice(0, 7)}
					</span>
				{/if}
				{#if pathState.kind === 'deleted'}
					<span
						class="ml-1 inline-flex items-center gap-1 rounded bg-warning/15 px-1.5 py-0.5 text-warning"
					>
						<TriangleAlert size={11} aria-hidden="true" />
						deleted
					</span>
				{/if}
			</div>
			<div class="ml-auto flex shrink-0 items-center gap-1.5">
				{#if pathState.kind === 'loading'}
					<span class="text-xs text-text-tertiary">Checking…</span>
				{:else if (pathState.kind === 'exists' || pathState.kind === 'unknown') && editUrl}
					{@const targetUrl = editUrl}
					{#if pathState.kind === 'unknown'}
						<TriangleAlert
							size={11}
							class="text-text-tertiary"
							aria-hidden="true"
							title="Couldn't verify the script still exists"
						/>
					{/if}
					<Button variant="ghost" size="sm" onclick={() => void goto(targetUrl)}>
						Edit
						<ExternalLink size={12} aria-hidden="true" />
					</Button>
				{/if}
			</div>
		{/if}
	</div>

	<!--
		Code viewer: reuses Monaco via ScriptEditor in readonly mode so font,
		theme, and find-shortcut (Ctrl+F) match the script editor exactly. We
		don't expose toolbar/requirements panel; this is purely a viewer.
	-->
	<!-- flex flex-col so ScriptEditor's flex-1 root fills this area; a plain block
	     parent leaves flex-1 with nothing to expand against and the editor
	     collapses to ~toolbar height (Monaco renders ~5px tall, looks blank). -->
	<div class="flex min-h-0 flex-1 flex-col">
		{#if codeContent !== null}
			<!-- No {#key run.id}: ScriptEditor syncs `content`/`language` props into
			     Monaco reactively, so navigating between runs updates in place rather
			     than tearing down and rebuilding Monaco (~300ms) each time. -->
			<ScriptEditor
				content={codeContent}
				language={editorLang}
				readonly={true}
				showRequirements={false}
				showRuntime={false}
			/>
		{:else}
			<div class="flex h-full items-center justify-center text-sm text-text-tertiary">
				No code recorded for this run.
			</div>
		{/if}
	</div>
</div>
