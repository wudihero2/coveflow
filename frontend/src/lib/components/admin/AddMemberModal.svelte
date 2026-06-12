<script lang="ts">
	import Button from '$lib/components/common/Button.svelte';
	import Modal from '$lib/components/common/Modal.svelte';
	import Select from '$lib/components/common/Select.svelte';
	import UserSearchInput from './UserSearchInput.svelte';
	import { WORKSPACE_ROLE_OPTIONS } from '$lib/constants';
	import type { WorkspaceRole } from '$lib/types';

	interface Props {
		open: boolean;
		existingEmails: string[];
		onConfirm: (email: string, role: WorkspaceRole) => void | Promise<void>;
	}

	let { open = $bindable(false), existingEmails, onConfirm }: Props = $props();

	let selectedEmail = $state('');
	let selectedRole = $state<string>('editor');
	let submitting = $state(false);


	async function handleConfirm(): Promise<void> {
		if (!selectedEmail) return;
		submitting = true;
		try {
			await onConfirm(selectedEmail, selectedRole as WorkspaceRole);
			open = false;
		} finally {
			submitting = false;
		}
	}

	$effect(() => {
		if (!open) {
			selectedEmail = '';
			selectedRole = 'editor';
		}
	});
</script>

<Modal bind:open title="Add Member">
	<div class="space-y-4">
		<div>
			<span class="block text-sm font-medium text-text-secondary">Email</span>
			<div class="mt-2">
				<UserSearchInput bind:value={selectedEmail} exclude={existingEmails} />
			</div>
		</div>
		<Select label="Role" options={WORKSPACE_ROLE_OPTIONS} bind:value={selectedRole} />
	</div>

	{#snippet actions()}
		<Button variant="secondary" onclick={() => (open = false)} disabled={submitting}>
			Cancel
		</Button>
		<Button variant="primary" onclick={handleConfirm} loading={submitting} disabled={!selectedEmail}>
			Add member
		</Button>
	{/snippet}
</Modal>
