<script lang="ts">
	import { Check, Copy, History, Pencil, Plus, Trash2 } from '@lucide/svelte';
	import { goto } from '$app/navigation';

	import Button from '$lib/components/common/Button.svelte';
	import IconButton from '$lib/components/common/IconButton.svelte';
	import WebhookFormModal from '$lib/components/triggers/WebhookFormModal.svelte';
	import { api, ApiClientError } from '$lib/services/api';
	import { confirmDialog } from '$lib/stores/confirm.svelte';
	import { workspace } from '$lib/stores/workspace.svelte';
	import { toastError, toastSuccess } from '$lib/toast';
	import type { Trigger } from '$lib/types';

	interface Props {
		flowId: string;
		/** Whether the user can create/edit/delete (write on the flow). */
		canManage: boolean;
	}

	let { flowId, canManage }: Props = $props();

	let rows = $state<Trigger[]>([]);
	let loading = $state(false);
	let loadError = $state('');
	let copiedId = $state('');

	let formOpen = $state(false);
	let editing = $state<Trigger | null>(null);

	let loadedKey = '';
	$effect(() => {
		const ws = workspace.id;
		if (!ws || !flowId) return;
		const key = `${ws}|${flowId}`;
		if (loadedKey === key) return;
		loadedKey = key;
		void load(ws);
	});

	async function load(ws: string): Promise<void> {
		loading = true;
		loadError = '';
		try {
			const all = await api.forWorkspace(ws).listTriggers(flowId);
			if (workspace.id !== ws) return;
			rows = all.filter((t) => t.trigger_type === 'webhook');
		} catch (e) {
			loadError = fmtErr(e);
		} finally {
			loading = false;
		}
	}

	function reload(): void {
		loadedKey = '';
		void load(workspace.id);
	}

	function fmtErr(e: unknown): string {
		return e instanceof ApiClientError ? `${e.status}: ${e.body || e.message}` : String(e);
	}

	// The full URL external systems POST to (the backend returns a relative path).
	function fullUrl(t: Trigger): string {
		const origin = typeof window !== 'undefined' ? window.location.origin : '';
		return `${origin}${t.webhook_path}`;
	}

	async function copyUrl(t: Trigger): Promise<void> {
		try {
			await navigator.clipboard.writeText(fullUrl(t));
			copiedId = t.id;
			setTimeout(() => (copiedId === t.id ? (copiedId = '') : null), 1500);
		} catch {
			toastError('Could not copy to clipboard');
		}
	}

	async function toggle(t: Trigger, enabled: boolean): Promise<void> {
		try {
			await api.forWorkspace(workspace.id).updateTrigger(t.id, { enabled });
		} catch (e) {
			toastError(fmtErr(e));
		}
		reload();
	}

	function openCreate(): void {
		editing = null;
		formOpen = true;
	}
	function openEdit(t: Trigger): void {
		editing = t;
		formOpen = true;
	}
	function onSaved(): void {
		formOpen = false;
		editing = null;
		reload();
	}

	async function remove(t: Trigger): Promise<void> {
		const ok = await confirmDialog({
			title: `Delete "${t.name}"?`,
			message: 'Calls to this webhook URL will stop working. Past runs are kept.',
			confirmLabel: 'Delete',
			variant: 'danger'
		});
		if (!ok) return;
		try {
			await api.forWorkspace(workspace.id).deleteTrigger(t.id);
			toastSuccess('Deleted');
			reload();
		} catch (e) {
			toastError(fmtErr(e));
		}
	}

	function viewRuns(t: Trigger): void {
		void goto(`/runs?trigger_id=${encodeURIComponent(t.id)}`);
	}
</script>

<div class="flex flex-col gap-3">
	{#if loadError}
		<p class="text-sm text-error">{loadError}</p>
	{:else if loading}
		<p class="text-sm text-text-tertiary">Loading…</p>
	{:else if rows.length === 0}
		<p class="text-sm text-text-tertiary">
			No webhooks yet. Create one to trigger this flow over HTTP.
		</p>
	{:else}
		<ul class="flex flex-col gap-2">
			{#each rows as t (t.id)}
				<li class="flex flex-col gap-2 rounded border border-border bg-surface-alt px-3 py-2.5">
					<div class="flex items-center gap-2">
						<span class="min-w-0 flex-1 truncate text-sm font-medium text-text">{t.name}</span>
						{#if canManage}
							<input
								type="checkbox"
								checked={t.enabled}
								aria-label="Enabled"
								class="size-4 cursor-pointer accent-accent"
								onchange={(e) => toggle(t, e.currentTarget.checked)}
							/>
						{:else if !t.enabled}
							<span class="text-xs text-text-tertiary">disabled</span>
						{/if}
						<IconButton aria-label="Run history" onclick={() => viewRuns(t)}>
							<History size={15} />
						</IconButton>
						{#if canManage}
							<IconButton aria-label="Edit" onclick={() => openEdit(t)}><Pencil size={15} /></IconButton>
							<IconButton aria-label="Delete" onclick={() => remove(t)}>
								<Trash2 size={15} class="text-error" />
							</IconButton>
						{/if}
					</div>
					<!-- The URL is the hero: monospace, full width, one-click copy. -->
					<div class="flex items-center gap-2 rounded bg-surface px-2 py-1.5">
						<code class="min-w-0 flex-1 truncate font-mono text-xs text-text-secondary" title={fullUrl(t)}>
							POST {fullUrl(t)}
						</code>
						<IconButton aria-label="Copy URL" onclick={() => copyUrl(t)}>
							{#if copiedId === t.id}
								<Check size={14} class="text-success" />
							{:else}
								<Copy size={14} />
							{/if}
						</IconButton>
					</div>
				</li>
			{/each}
		</ul>
	{/if}

	{#if canManage}
		<div>
			<Button variant="secondary" onclick={openCreate}>
				<Plus size={15} /> New webhook
			</Button>
		</div>
	{/if}

	<p class="text-xs text-text-tertiary">
		Call with header <code>Authorization: Bearer &lt;your API token&gt;</code> (Settings → API
		Tokens). The JSON body becomes the flow input; the run executes as you.
	</p>
</div>

<WebhookFormModal bind:open={formOpen} {flowId} {editing} onClose={() => (formOpen = false)} {onSaved} />
