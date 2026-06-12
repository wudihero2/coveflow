<script lang="ts">
	import { Plus } from '@lucide/svelte';

	import Alert from '$lib/components/common/Alert.svelte';
	import Button from '$lib/components/common/Button.svelte';
	import DataTable from '$lib/components/common/DataTable.svelte';
	import PageFrame from '$lib/components/common/PageFrame.svelte';
	import Select from '$lib/components/common/Select.svelte';
	import AddMemberModal from '$lib/components/admin/AddMemberModal.svelte';
	import ConfirmModal from '$lib/components/admin/ConfirmModal.svelte';
	import { WORKSPACE_ROLE_OPTIONS } from '$lib/constants';
	import { api, ApiClientError } from '$lib/services/api';
	import { useWorkspaceLoader } from '$lib/services/workspace-loader.svelte';
	import { auth } from '$lib/stores/auth.svelte';
	import { workspace } from '$lib/stores/workspace.svelte';
	import { toastError, toastSuccess } from '$lib/toast';
	import type { WorkspaceMember, WorkspaceRole } from '$lib/types';

	type MemberRow = WorkspaceMember & { actions: '' };

	const memberLoader = useWorkspaceLoader<WorkspaceMember[]>((ws) => ws.listMembers());

	const currentEmail = $derived(auth.email ?? '');

	const columns = [
		{ key: 'email', label: 'Email', gridTrack: 'minmax(200px,1fr)' },
		{ key: 'role', label: 'Role', gridTrack: '160px' },
		{ key: 'actions', label: 'Actions', gridTrack: '100px' }
	] satisfies { key: keyof MemberRow; label: string; gridTrack?: string }[];

	const rows = $derived<MemberRow[]>(
		(memberLoader.data ?? []).map((m) => ({ ...m, actions: '' as const }))
	);

	// Add member modal
	let addOpen = $state(false);
	const existingEmails = $derived((memberLoader.data ?? []).map((m) => m.email));

	async function handleAddMember(email: string, role: WorkspaceRole): Promise<void> {
		const wsId = workspace.id;
		try {
			await api.forWorkspace(wsId).addMember(email, role);
			toastSuccess(`Added ${email}`);
			memberLoader.reload();
		} catch (e) {
			toastError(e instanceof ApiClientError ? e.body || e.message : String(e));
			throw e;
		}
	}

	// Role change
	async function handleRoleChange(email: string, newRole: string): Promise<void> {
		const wsId = workspace.id;
		try {
			await api.forWorkspace(wsId).updateMemberRole(email, newRole as WorkspaceRole);
			toastSuccess(`Updated ${email} to ${newRole}`);
			memberLoader.reload();
		} catch (e) {
			toastError(e instanceof ApiClientError ? e.body || e.message : String(e));
			memberLoader.reload();
		}
	}

	// Remove member
	let removeTarget = $state('');
	let removeOpen = $state(false);

	function startRemove(email: string): void {
		removeTarget = email;
		removeOpen = true;
	}

	async function handleRemove(): Promise<void> {
		const wsId = workspace.id;
		try {
			await api.forWorkspace(wsId).removeMember(removeTarget);
			toastSuccess(`Removed ${removeTarget}`);
			memberLoader.reload();
		} catch (e) {
			toastError(e instanceof ApiClientError ? e.body || e.message : String(e));
			throw e;
		}
	}
</script>

<svelte:head>
	<title>Members | CoveFlow</title>
</svelte:head>

<PageFrame title="Members" subtitle="Manage workspace members">
	{#snippet actions()}
		<Button variant="primary" onclick={() => (addOpen = true)}>
			<Plus size={16} />
			Add Member
		</Button>
	{/snippet}

	{#if memberLoader.error}
		<Alert variant="error">{memberLoader.error}</Alert>
	{:else}
		<div class="mt-6">
			<DataTable
				{columns}
				{rows}
				loading={memberLoader.loading}
				emptyText="No members yet. Add one to get started."
			>
				{#snippet cell({ row, column })}
					{#if column.key === 'email'}
						<span class="text-text">{row.email}</span>
					{:else if column.key === 'role'}
						{#if row.email === currentEmail}
							<span class="text-text-secondary">{row.role}</span>
						{:else}
							<!-- Fixed-width wrapper: keep the select narrow so it doesn't
							     stretch the full Role column and crowd the Actions cell. -->
							<div class="w-28">
								<Select
									options={WORKSPACE_ROLE_OPTIONS}
									value={row.role}
									onchange={(v) => void handleRoleChange(row.email, v)}
									compact
								/>
							</div>
						{/if}
					{:else if column.key === 'actions'}
						{#if row.email !== currentEmail}
							<Button variant="ghost" size="sm" onclick={() => startRemove(row.email)}>
								Remove
							</Button>
						{/if}
					{/if}
				{/snippet}
			</DataTable>
		</div>
	{/if}
</PageFrame>

<AddMemberModal bind:open={addOpen} {existingEmails} onConfirm={handleAddMember} />

<ConfirmModal
	bind:open={removeOpen}
	title="Remove Member"
	message="Remove {removeTarget} from this workspace? They will also be removed from all teams."
	confirmLabel="Remove"
	onConfirm={handleRemove}
/>
