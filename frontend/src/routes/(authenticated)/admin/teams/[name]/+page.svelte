<script lang="ts">
	import { ArrowLeft, Plus, Trash2 } from '@lucide/svelte';
	import { page } from '$app/state';
	import { goto } from '$app/navigation';

	import Alert from '$lib/components/common/Alert.svelte';
	import Button from '$lib/components/common/Button.svelte';
	import Card from '$lib/components/common/Card.svelte';
	import PageFrame from '$lib/components/common/PageFrame.svelte';
	import TextInput from '$lib/components/common/TextInput.svelte';
	import Select from '$lib/components/common/Select.svelte';
	import AddTeamMemberModal from '$lib/components/admin/AddTeamMemberModal.svelte';
	import ConfirmModal from '$lib/components/admin/ConfirmModal.svelte';
	import { api, ApiClientError } from '$lib/services/api';
	import { useWorkspaceLoader } from '$lib/services/workspace-loader.svelte';
	import { workspace } from '$lib/stores/workspace.svelte';
	import { toastError, toastSuccess } from '$lib/toast';
	import type { TeamListItem, TeamMember, TeamQuota, WorkspaceMember } from '$lib/types';

	const teamRoleOptions = [
		{ value: 'writer', label: 'Writer (read + edit)' },
		{ value: 'reader', label: 'Reader (view only)' }
	];

	const teamName = $derived(page.params.name ?? '');

	// Load team info (for summary display)
	const teamInfoLoader = useWorkspaceLoader<TeamListItem>(
		(ws) => ws.getTeam(teamName),
		{ key: () => teamName, enabled: () => !!teamName }
	);

	const teamSummary = $derived(teamInfoLoader.data?.summary ?? '');

	// Load team members
	const membersLoader = useWorkspaceLoader<TeamMember[]>(
		(ws) => ws.listTeamMembers(teamName),
		{ key: () => teamName, enabled: () => !!teamName }
	);

	// Load workspace members lazily — only when the "add member" modal opens
	let addMemberOpen = $state(false);
	const wsMembersLoader = useWorkspaceLoader<WorkspaceMember[]>((ws) => ws.listMembers(), {
		enabled: () => addMemberOpen
	});

	// Load quota
	const quotaLoader = useWorkspaceLoader<TeamQuota>(
		(ws) => ws.getTeamQuota(teamName),
		{ key: () => teamName, enabled: () => !!teamName }
	);

	// Quota editing state
	let quotaForm = $state<Record<string, string>>({
		max_concurrent_runs: '',
		max_cpus: '',
		max_memory_mb: '',
		max_daily_runs: '',
		max_storage_bytes: '',
		max_run_timeout_secs: ''
	});
	let quotaSaving = $state(false);
	let quotaDirty = $state(false);

	const emptyQuotaForm: Record<string, string> = {
		max_concurrent_runs: '',
		max_cpus: '',
		max_memory_mb: '',
		max_daily_runs: '',
		max_storage_bytes: '',
		max_run_timeout_secs: ''
	};

	$effect(() => {
		if (quotaLoader.loading || quotaDirty) return;
		const q = quotaLoader.data;
		if (q) {
			quotaForm = {
				max_concurrent_runs: q.max_concurrent_runs?.toString() ?? '',
				max_cpus: q.max_cpus?.toString() ?? '',
				max_memory_mb: q.max_memory_mb?.toString() ?? '',
				max_daily_runs: q.max_daily_runs?.toString() ?? '',
				max_storage_bytes: q.max_storage_bytes?.toString() ?? '',
				max_run_timeout_secs: q.max_run_timeout_secs?.toString() ?? ''
			};
		} else {
			quotaForm = { ...emptyQuotaForm };
		}
	});

	async function saveQuota(): Promise<void> {
		const wsId = workspace.id;
		quotaSaving = true;
		try {
			const quota: TeamQuota = {
				max_concurrent_runs: quotaForm.max_concurrent_runs ? Number(quotaForm.max_concurrent_runs) : null,
				max_cpus: quotaForm.max_cpus ? Number(quotaForm.max_cpus) : null,
				max_memory_mb: quotaForm.max_memory_mb ? Number(quotaForm.max_memory_mb) : null,
				max_daily_runs: quotaForm.max_daily_runs ? Number(quotaForm.max_daily_runs) : null,
				max_storage_bytes: quotaForm.max_storage_bytes ? Number(quotaForm.max_storage_bytes) : null,
				max_run_timeout_secs: quotaForm.max_run_timeout_secs ? Number(quotaForm.max_run_timeout_secs) : null
			};
			await api.forWorkspace(wsId).updateTeamQuota(teamName, quota);
			toastSuccess('Quota saved');
			quotaDirty = false;
			quotaLoader.reload();
		} catch (e) {
			toastError(e instanceof ApiClientError ? e.body || e.message : String(e));
		} finally {
			quotaSaving = false;
		}
	}

	// Add member modal — state declared above with wsMembersLoader

	async function handleAddMember(email: string, role: 'reader' | 'writer'): Promise<void> {
		const wsId = workspace.id;
		try {
			await api.forWorkspace(wsId).addTeamMember(teamName, email, role);
			toastSuccess(`Added ${email}`);
			membersLoader.reload();
		} catch (e) {
			toastError(e instanceof ApiClientError ? e.body || e.message : String(e));
			throw e;
		}
	}

	async function handleUpdateRole(email: string, role: string): Promise<void> {
		const wsId = workspace.id;
		try {
			await api.forWorkspace(wsId).updateTeamMemberRole(teamName, email, role as 'reader' | 'writer');
			membersLoader.reload();
		} catch (e) {
			toastError(e instanceof ApiClientError ? e.body || e.message : String(e));
		}
	}

	// Remove member
	let removeMemberTarget = $state('');
	let removeMemberOpen = $state(false);

	async function handleRemoveMember(): Promise<void> {
		const wsId = workspace.id;
		try {
			await api.forWorkspace(wsId).removeTeamMember(teamName, removeMemberTarget);
			toastSuccess(`Removed ${removeMemberTarget}`);
			membersLoader.reload();
		} catch (e) {
			toastError(e instanceof ApiClientError ? e.body || e.message : String(e));
			throw e;
		}
	}

	// Delete team
	let deleteOpen = $state(false);

	async function handleDeleteTeam(): Promise<void> {
		const wsId = workspace.id;
		try {
			await api.forWorkspace(wsId).deleteTeam(teamName);
			toastSuccess(`Team "${teamName}" deleted`);
			void goto('/admin/teams', { replaceState: true });
		} catch (e) {
			toastError(e instanceof ApiClientError ? e.body || e.message : String(e));
			throw e;
		}
	}

	const quotaFields: Array<{ key: string; label: string }> = [
		{ key: 'max_concurrent_runs', label: 'Max Concurrent Runs' },
		{ key: 'max_cpus', label: 'Max CPUs' },
		{ key: 'max_memory_mb', label: 'Max Memory (MB)' },
		{ key: 'max_daily_runs', label: 'Max Daily Runs' },
		{ key: 'max_storage_bytes', label: 'Max Storage (bytes)' },
		{ key: 'max_run_timeout_secs', label: 'Max Run Timeout (s)' }
	];
</script>

<svelte:head>
	<title>{teamName} | Teams | CoveFlow</title>
</svelte:head>

<PageFrame title="Team: {teamName}" subtitle={teamSummary || undefined}>
	{#snippet actions()}
		<Button variant="danger" onclick={() => (deleteOpen = true)}>
			<Trash2 size={16} />
			Delete Team
		</Button>
	{/snippet}

	<div class="space-y-6 py-2">
		<a
			href="/admin/teams"
			class="inline-flex items-center gap-1 text-sm text-text-secondary transition hover:text-text"
		>
			<ArrowLeft size={14} />
			Back to teams
		</a>

		<!-- Members -->
		<Card title="Members">
			{#if membersLoader.error}
				<Alert variant="error">{membersLoader.error}</Alert>
			{:else if membersLoader.loading}
				<p class="text-sm text-text-tertiary">Loading…</p>
			{:else}
				<div class="mb-3 flex justify-end">
					<Button variant="secondary" size="sm" onclick={() => (addMemberOpen = true)}>
						<Plus size={14} />
						Add Member
					</Button>
				</div>
				{#if (membersLoader.data ?? []).length === 0}
					<p class="text-sm text-text-tertiary">No members yet.</p>
				{:else}
					<table class="w-full text-sm">
						<tbody>
							{#each membersLoader.data ?? [] as m (m.email)}
								<tr class="border-b border-border/50">
									<td class="py-2 text-text">{m.email}</td>
									<td class="py-2">
										<Select
											options={teamRoleOptions}
											value={m.role}
											onchange={(v) => void handleUpdateRole(m.email, v)}
										/>
									</td>
									<td class="py-2 text-right">
										<Button
											variant="ghost"
											size="sm"
											onclick={() => {
												removeMemberTarget = m.email;
												removeMemberOpen = true;
											}}
										>
											Remove
										</Button>
									</td>
								</tr>
							{/each}
						</tbody>
					</table>
				{/if}
			{/if}
		</Card>

		<!-- Quota -->
		<Card title="Quota">
			{#if quotaLoader.error}
				<Alert variant="error">{quotaLoader.error}</Alert>
			{:else if quotaLoader.loading}
				<p class="text-sm text-text-tertiary">Loading…</p>
			{:else}
				<div class="grid gap-4 sm:grid-cols-2">
					{#each quotaFields as field (field.key)}
						<TextInput
							label={field.label}
							type="number"
							placeholder="No limit"
							bind:value={quotaForm[field.key]}
							oninput={() => (quotaDirty = true)}
						/>
					{/each}
				</div>
				<div class="mt-4 flex justify-end">
					<Button variant="primary" onclick={saveQuota} loading={quotaSaving}>
						Save Quota
					</Button>
				</div>
			{/if}
		</Card>
	</div>
</PageFrame>

<AddTeamMemberModal
	bind:open={addMemberOpen}
	workspaceMembers={wsMembersLoader.data ?? []}
	existingEmails={(membersLoader.data ?? []).map((m) => m.email)}
	onConfirm={handleAddMember}
/>

<ConfirmModal
	bind:open={removeMemberOpen}
	title="Remove Team Member"
	message="Remove {removeMemberTarget} from team {teamName}?"
	confirmLabel="Remove"
	onConfirm={handleRemoveMember}
/>

<ConfirmModal
	bind:open={deleteOpen}
	title="Delete Team"
	message="This will permanently delete the team and all associated members, quotas, and ACL entries."
	confirmLabel="Delete team"
	onConfirm={handleDeleteTeam}
/>
