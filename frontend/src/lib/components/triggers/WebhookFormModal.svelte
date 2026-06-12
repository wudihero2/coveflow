<script lang="ts">
	import Button from '$lib/components/common/Button.svelte';
	import Modal from '$lib/components/common/Modal.svelte';
	import TextInput from '$lib/components/common/TextInput.svelte';
	import { api, ApiClientError } from '$lib/services/api';
	import { workspace } from '$lib/stores/workspace.svelte';
	import { toastSuccess } from '$lib/toast';
	import type { Trigger } from '$lib/types';

	interface Props {
		open: boolean;
		flowId: string;
		/** Existing webhook to edit, or null to create. */
		editing: Trigger | null;
		onClose: () => void;
		onSaved: () => void;
	}

	let { open = $bindable(false), flowId, editing, onClose, onSaved }: Props = $props();

	let name = $state('');
	let maxActive = $state('');
	let saving = $state(false);
	let error = $state('');

	// Seed the form whenever it opens (create = blank, edit = existing values).
	$effect(() => {
		if (open) {
			name = editing?.name ?? '';
			const m = editing?.config?.max_active_runs;
			maxActive = typeof m === 'number' ? String(m) : '';
			error = '';
			saving = false;
		}
	});

	const canSave = $derived(!saving && name.trim().length > 0);

	function buildConfig(): Record<string, unknown> {
		const trimmed = maxActive.trim();
		if (trimmed === '') return {};
		const n = Number(trimmed);
		return Number.isFinite(n) && n >= 1 ? { max_active_runs: Math.floor(n) } : {};
	}

	async function save(): Promise<void> {
		if (!canSave) return;
		saving = true;
		error = '';
		const ws = api.forWorkspace(workspace.id);
		try {
			if (editing) {
				await ws.updateTrigger(editing.id, { name: name.trim(), config: buildConfig() });
				toastSuccess('Webhook updated');
			} else {
				await ws.createTrigger(flowId, {
					name: name.trim(),
					trigger_type: 'webhook',
					config: buildConfig()
				});
				toastSuccess('Webhook created');
			}
			onSaved();
		} catch (e) {
			error = e instanceof ApiClientError ? `${e.status}: ${e.body || e.message}` : String(e);
		} finally {
			saving = false;
		}
	}
</script>

<Modal {open} title={editing ? 'Edit webhook' : 'New webhook'}>
	<div class="flex flex-col gap-4">
		<TextInput bind:value={name} label="Name" placeholder="e.g. github-push" mono />
		<div>
			<TextInput
				bind:value={maxActive}
				label="Max active runs (optional)"
				placeholder="unlimited"
				type="number"
			/>
			<p class="mt-1 text-xs text-text-tertiary">
				Reject new calls (429) while this many runs are still in flight. Blank = unlimited.
			</p>
		</div>
		{#if error}
			<p class="text-sm text-error">{error}</p>
		{/if}
	</div>

	{#snippet actions()}
		<Button variant="ghost" onclick={onClose}>Cancel</Button>
		<Button variant="primary" loading={saving} disabled={!canSave} onclick={save}>
			{editing ? 'Save' : 'Create'}
		</Button>
	{/snippet}
</Modal>
