<script lang="ts">
	import { Eye, LoaderCircle, RotateCcw } from '@lucide/svelte';
	import Alert from '$lib/components/common/Alert.svelte';
	import Button from '$lib/components/common/Button.svelte';
	import IconButton from '$lib/components/common/IconButton.svelte';
	import { api, ApiClientError } from '$lib/services/api';
	import { workspace } from '$lib/stores/workspace.svelte';
	import { toastError } from '$lib/toast';
	import type { ScriptResponse, ScriptVersionItem } from '$lib/types';
	import { formatRelative } from '$lib/utils/format-time';

	const PAGE_SIZE = 20;

	interface Props {
		path: string;
		/** Hash of the currently loaded script; used to mark the matching row as "current". */
		currentHash: string;
		onView: (script: ScriptResponse) => void;
		onRestore: (script: ScriptResponse) => void;
		class?: string;
	}

	let { path, currentHash, onView, onRestore, class: className = '' }: Props = $props();

	let items = $state.raw<ScriptVersionItem[]>([]);
	let total = $state(0);
	let hasMore = $state(false);
	let loading = $state(false);
	let loadingMore = $state(false);
	let error = $state('');
	/** Hash currently being fetched for View / Restore — disables that row's buttons. */
	let busyHash = $state<string | null>(null);

	// Re-fetch from page 0 whenever the path or active workspace changes.
	$effect(() => {
		const wsId = workspace.id;
		const p = path;
		if (!wsId || !p) return;
		void loadFirstPage(wsId, p);
	});

	async function loadFirstPage(wsId: string, p: string): Promise<void> {
		loading = true;
		error = '';
		items = [];
		total = 0;
		hasMore = false;
		try {
			const response = await api.forWorkspace(wsId).listScriptVersions(p, PAGE_SIZE, 0);
			// Guard against workspace/path drift while the request was in flight.
			if (workspace.id !== wsId || path !== p) return;
			items = response.items;
			total = response.total;
			hasMore = response.has_more;
		} catch (e) {
			if (workspace.id !== wsId || path !== p) return;
			error = e instanceof ApiClientError ? `${e.status}: ${e.body || e.message}` : String(e);
		} finally {
			if (workspace.id === wsId && path === p) loading = false;
		}
	}

	async function loadMore(): Promise<void> {
		const wsId = workspace.id;
		const p = path;
		if (loadingMore || !hasMore) return;
		loadingMore = true;
		try {
			const response = await api
				.forWorkspace(wsId)
				.listScriptVersions(p, PAGE_SIZE, items.length);
			if (workspace.id !== wsId || path !== p) return;
			items = [...items, ...response.items];
			hasMore = response.has_more;
		} catch (e) {
			if (workspace.id !== wsId || path !== p) return;
			toastError(
				e instanceof ApiClientError ? `${e.status}: ${e.body || e.message}` : String(e)
			);
		} finally {
			if (workspace.id === wsId && path === p) loadingMore = false;
		}
	}

	async function fetchScript(hash: string): Promise<ScriptResponse | null> {
		const wsId = workspace.id;
		busyHash = hash;
		try {
			return await api.forWorkspace(wsId).getScriptByHash(hash);
		} catch (e) {
			toastError(
				e instanceof ApiClientError ? `${e.status}: ${e.body || e.message}` : String(e)
			);
			return null;
		} finally {
			busyHash = null;
		}
	}

	async function handleView(hash: string): Promise<void> {
		const script = await fetchScript(hash);
		if (script) onView(script);
	}

	async function handleRestore(hash: string): Promise<void> {
		const script = await fetchScript(hash);
		if (script) onRestore(script);
	}

</script>

<div class="flex min-h-0 flex-col {className}">
	<div class="flex items-center gap-2 border-b border-border bg-surface-alt px-3 py-1.5 text-xs">
		<!--
			No History icon here: the parent Tab bar already labels this panel.
			Repeating the same glyph 14px / 12px in two adjacent rows adds visual
			noise without adding information. LogViewer keeps its status icon
			because the icon there changes with state (connecting/streaming/...).
		-->
		{#if loading}
			<LoaderCircle size={12} class="animate-spin text-text-tertiary" />
			<span class="text-text-tertiary">Loading versions…</span>
		{:else}
			<span class="font-medium text-text">
				{total}
				{total === 1 ? 'version' : 'versions'}
			</span>
		{/if}
	</div>

	<div class="min-h-0 flex-1 overflow-y-auto">
		{#if error}
			<div class="p-3">
				<Alert variant="error">{error}</Alert>
			</div>
		{:else if loading && items.length === 0}
			<div class="flex h-full items-center justify-center gap-2 text-sm text-text-tertiary">
				<LoaderCircle size={16} class="animate-spin" />
				<span>Loading…</span>
			</div>
		{:else if total === 0}
			<div class="flex h-full items-center justify-center text-sm text-text-tertiary">
				No versions yet
			</div>
		{:else}
			<ul class="divide-y divide-border">
				{#each items as item (item.hash)}
					{@const isCurrent = item.hash === currentHash}
					{@const isBusy = busyHash === item.hash}
					<li class="flex items-start gap-3 px-3 py-2 text-sm hover:bg-surface-alt/40">
						<!--
							Information hierarchy: developers scan by *what changed* (summary),
							not by hash. So summary leads (bold, primary text), and hash drops
							into the metadata line alongside author + time.
						-->
						<div class="min-w-0 flex-1">
							<div class="flex items-center gap-2">
								<div class="min-w-0 flex-1 truncate font-medium text-text" title={item.summary || ''}>
									{item.summary || 'Untitled version'}
								</div>
								{#if isCurrent}
									<span
										class="shrink-0 rounded bg-accent/15 px-1.5 py-0.5 text-[10px] font-medium uppercase tracking-wider text-accent"
									>
										current
									</span>
								{/if}
							</div>
							<div class="mt-0.5 flex items-center gap-2 text-xs text-text-tertiary">
								<code class="font-mono text-text-secondary">{item.hash.slice(0, 8)}</code>
								<span>·</span>
								<span class="truncate">{item.created_by}</span>
								<span>·</span>
								<time datetime={item.created_at} title={item.created_at}>
									{formatRelative(item.created_at)}
								</time>
							</div>
						</div>
						<div class="flex shrink-0 gap-1">
							<IconButton
								aria-label="View this version"
								title="View"
								onclick={() => void handleView(item.hash)}
								disabled={isBusy}
							>
								<Eye size={14} />
							</IconButton>
							<IconButton
								aria-label={isCurrent
									? 'Reload current version into editor'
									: 'Restore this version'}
								title={isCurrent ? 'Reload current into editor' : 'Restore'}
								onclick={() => void handleRestore(item.hash)}
								disabled={isBusy}
							>
								<RotateCcw size={14} />
							</IconButton>
						</div>
					</li>
				{/each}
			</ul>

			{#if hasMore}
				<div class="flex justify-center p-3">
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
		{/if}
	</div>
</div>
