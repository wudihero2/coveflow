<script lang="ts">
	import Button from '$lib/components/common/Button.svelte';
	import Modal from '$lib/components/common/Modal.svelte';
	import Select from '$lib/components/common/Select.svelte';
	import type { WorkspaceMember } from '$lib/types';

	interface Props {
		open: boolean;
		workspaceMembers: WorkspaceMember[];
		existingEmails: string[];
		onConfirm: (email: string, role: 'reader' | 'writer') => void | Promise<void>;
	}

	let { open = $bindable(false), workspaceMembers, existingEmails, onConfirm }: Props = $props();

	let selectedEmail = $state('');
	let selectedRole = $state<'reader' | 'writer'>('writer');
	let submitting = $state(false);

	const roleOptions = [
		{ value: 'writer', label: 'Writer (read + edit)' },
		{ value: 'reader', label: 'Reader (view only)' }
	];

	const availableOptions = $derived.by(() => {
		const existing = new Set(existingEmails);
		return workspaceMembers
			.filter((m) => !existing.has(m.email))
			.sort((a, b) => a.email.localeCompare(b.email))
			.map((m) => ({ value: m.email, label: `${m.email} (${m.role})` }));
	});

	const selectOptions = $derived([
		{ value: '', label: availableOptions.length > 0 ? 'Select a member…' : 'No members available' },
		...availableOptions
	]);

	async function handleConfirm(): Promise<void> {
		if (!selectedEmail) return;
		submitting = true;
		try {
			await onConfirm(selectedEmail, selectedRole);
			open = false;
		} finally {
			submitting = false;
		}
	}

	$effect(() => {
		if (!open) {
			selectedEmail = '';
			selectedRole = 'writer';
		}
	});
</script>

<Modal bind:open title="Add Team Member">
	{#if availableOptions.length === 0}
		<p class="text-sm text-text-secondary">All workspace members are already in this team.</p>
	{:else}
		<div class="space-y-3">
			<Select label="Workspace member" options={selectOptions} bind:value={selectedEmail} />
			<Select label="Role" options={roleOptions} bind:value={selectedRole} />
		</div>
	{/if}

	{#snippet actions()}
		<Button variant="secondary" onclick={() => (open = false)} disabled={submitting}>
			Cancel
		</Button>
		<Button
			variant="primary"
			onclick={handleConfirm}
			loading={submitting}
			disabled={!selectedEmail || availableOptions.length === 0}
		>
			Add
		</Button>
	{/snippet}
</Modal>
