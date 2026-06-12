<script lang="ts">
	import { Play, Pencil, Trash2, History, Plus, TriangleAlert } from '@lucide/svelte';
	import { goto } from '$app/navigation';

	import Badge from '$lib/components/common/Badge.svelte';
	import Button from '$lib/components/common/Button.svelte';
	import IconButton from '$lib/components/common/IconButton.svelte';
	import Modal from '$lib/components/common/Modal.svelte';
	import ScheduleFormModal from '$lib/components/schedules/ScheduleFormModal.svelte';
	import WebhooksPanel from '$lib/components/triggers/WebhooksPanel.svelte';
	import { api, ApiClientError } from '$lib/services/api';
	import { confirmDialog } from '$lib/stores/confirm.svelte';
	import { workspace } from '$lib/stores/workspace.svelte';
	import { displayTz } from '$lib/stores/timezone.svelte';
	import { formatAbsolute } from '$lib/utils/format-time';
	import { toastError, toastSuccess } from '$lib/toast';
	import type { FlowListItem, Schedule, ScheduleListItem } from '$lib/types';

	interface Props {
		open: boolean;
		/** Stable id of the flow whose schedules these are. */
		flowId: string;
		/** Current path of the flow, for display. */
		flowPath: string;
		/** Whether the user can create/edit/delete (write on the flow). */
		canManage: boolean;
		onClose: () => void;
	}

	let { open, flowId, flowPath, canManage, onClose }: Props = $props();

	let tab = $state<'schedules' | 'webhooks'>('schedules');

	let rows = $state<ScheduleListItem[]>([]);
	let loading = $state(false);
	let loadError = $state('');

	// Create/edit form stacks over this modal.
	let formOpen = $state(false);
	let editing = $state<Schedule | null>(null);

	// The flow as a single-item list for the form's (disabled) flow picker.
	const flowAsList = $derived<FlowListItem[]>([
		{ flow_id: flowId, path: flowPath, revision: 0, summary: '', edited_by: '', edited_at: '' }
	]);

	let loadedKey = '';
	$effect(() => {
		const ws = workspace.id;
		if (!open || !ws || !flowId) return;
		const key = `${ws}|${flowId}`;
		if (loadedKey === key) return;
		loadedKey = key;
		void load();
	});
	$effect(() => {
		if (!open) loadedKey = '';
	});

	async function load(): Promise<void> {
		loading = true;
		loadError = '';
		try {
			const all = await api.forWorkspace(workspace.id).listSchedules();
			rows = all.filter((s) => s.flow_id === flowId);
		} catch (e) {
			loadError = e instanceof ApiClientError ? `${e.status}: ${e.body || e.message}` : String(e);
		} finally {
			loading = false;
		}
	}

	function reload(): void {
		loadedKey = '';
		if (open) void load();
	}

	function fmtErr(e: unknown): string {
		return e instanceof ApiClientError ? `${e.status}: ${e.body || e.message}` : String(e);
	}

	function whenLabel(ts: string | null): string {
		return formatAbsolute(ts, displayTz.value);
	}

	function lastRun(s: ScheduleListItem): { label: string; variant: 'success' | 'error' | 'info' | 'ghost' } {
		if (!s.last_run) return { label: 'never', variant: 'ghost' };
		if (s.last_run.success === null) return { label: 'running', variant: 'info' };
		return s.last_run.success
			? { label: 'success', variant: 'success' }
			: { label: 'failed', variant: 'error' };
	}

	function openCreate(): void {
		editing = null;
		formOpen = true;
	}

	async function openEdit(s: ScheduleListItem): Promise<void> {
		try {
			editing = await api.forWorkspace(workspace.id).getSchedule(s.id);
			formOpen = true;
		} catch (e) {
			toastError(fmtErr(e));
		}
	}

	function onSaved(): void {
		formOpen = false;
		editing = null;
		reload();
	}

	async function toggle(s: ScheduleListItem, enabled: boolean): Promise<void> {
		try {
			await api.forWorkspace(workspace.id).setScheduleEnabled(s.id, enabled);
		} catch (e) {
			toastError(fmtErr(e));
		}
		reload();
	}

	async function runNow(s: ScheduleListItem): Promise<void> {
		try {
			await api.forWorkspace(workspace.id).runScheduleNow(s.id);
			toastSuccess('Triggered');
			reload();
		} catch (e) {
			toastError(fmtErr(e));
		}
	}

	async function remove(s: ScheduleListItem): Promise<void> {
		const ok = await confirmDialog({
			title: `Delete "${s.name}"?`,
			message: 'This removes the schedule. Past runs are kept.',
			confirmLabel: 'Delete',
			variant: 'danger'
		});
		if (!ok) return;
		try {
			await api.forWorkspace(workspace.id).deleteSchedule(s.id);
			toastSuccess('Deleted');
			reload();
		} catch (e) {
			toastError(fmtErr(e));
		}
	}

	// History = the Runs page pre-filtered to this schedule's runs.
	function viewRuns(s: ScheduleListItem): void {
		void goto(`/runs?schedule_id=${encodeURIComponent(s.id)}`);
	}
</script>

<Modal {open} title="Triggers · {flowPath.split('/').pop()}" size="xl">
	<div class="mb-3 flex gap-4 border-b border-border text-sm">
		<button
			type="button"
			class="-mb-px border-b-2 px-1 pb-2 {tab === 'schedules' ? 'border-accent text-text' : 'border-transparent text-text-tertiary hover:text-text'}"
			onclick={() => (tab = 'schedules')}>Schedules</button>
		<button
			type="button"
			class="-mb-px border-b-2 px-1 pb-2 {tab === 'webhooks' ? 'border-accent text-text' : 'border-transparent text-text-tertiary hover:text-text'}"
			onclick={() => (tab = 'webhooks')}>Webhooks</button>
	</div>
	{#if tab === 'webhooks'}
		<WebhooksPanel {flowId} {canManage} />
	{:else}
	<div class="flex flex-col gap-2">
		{#if loadError}
			<p class="text-sm text-error">{loadError}</p>
		{:else if loading}
			<p class="text-sm text-text-tertiary">Loading…</p>
		{:else if rows.length === 0}
			<p class="text-sm text-text-tertiary">No schedules for this flow yet.</p>
		{:else}
			<ul class="flex flex-col gap-1">
				{#each rows as s (s.id)}
					{@const lr = lastRun(s)}
					<li class="flex items-center gap-2 rounded border border-border bg-surface-alt px-3 py-2 text-sm">
						<span class="flex min-w-0 flex-1 flex-col">
							<span class="flex items-center gap-1.5 font-medium text-text">
								<span class="truncate">{s.name}</span>
								{#if s.last_error}
									<span title={s.last_error}><TriangleAlert size={13} class="text-warning" /></span>
								{/if}
							</span>
							<span class="truncate font-mono text-xs whitespace-nowrap text-text-tertiary">
								{s.cron_expr} · {s.enabled ? whenLabel(s.next_trigger_at) : 'paused'}
							</span>
						</span>
						<Badge variant={lr.variant}>{lr.label}</Badge>
						{#if canManage}
							<input
								type="checkbox"
								checked={s.enabled}
								aria-label="Enabled"
								class="size-4 cursor-pointer accent-accent"
								onchange={(e) => toggle(s, e.currentTarget.checked)}
							/>
							<IconButton aria-label="Run now" onclick={() => runNow(s)}><Play size={15} /></IconButton>
						{/if}
						<IconButton aria-label="Run history" onclick={() => viewRuns(s)}>
							<History size={15} />
						</IconButton>
						{#if canManage}
							<IconButton aria-label="Edit" onclick={() => openEdit(s)}><Pencil size={15} /></IconButton>
							<IconButton aria-label="Delete" onclick={() => remove(s)}>
								<Trash2 size={15} class="text-error" />
							</IconButton>
						{/if}
					</li>
				{/each}
			</ul>
		{/if}
	</div>
	{/if}

	{#snippet actions()}
		{#if canManage && tab === 'schedules'}
			<Button variant="secondary" onclick={openCreate}>
				<Plus size={15} /> New schedule
			</Button>
		{/if}
		<Button variant="primary" onclick={onClose}>Close</Button>
	{/snippet}
</Modal>

<ScheduleFormModal
	open={formOpen}
	schedule={editing}
	defaultFlowId={flowId}
	flows={flowAsList}
	onClose={() => (formOpen = false)}
	{onSaved}
/>
