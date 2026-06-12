<script lang="ts">
	import { Plus } from '@lucide/svelte';
	import { goto } from '$app/navigation';

	import Alert from '$lib/components/common/Alert.svelte';
	import Button from '$lib/components/common/Button.svelte';
	import DataTable from '$lib/components/common/DataTable.svelte';
	import PageFrame from '$lib/components/common/PageFrame.svelte';
	import CreateTeamModal from '$lib/components/admin/CreateTeamModal.svelte';
	import { api, ApiClientError } from '$lib/services/api';
	import { useWorkspaceLoader } from '$lib/services/workspace-loader.svelte';
	import { workspace } from '$lib/stores/workspace.svelte';
	import { toastError, toastSuccess } from '$lib/toast';
	import type { TeamListItem } from '$lib/types';

	const teamLoader = useWorkspaceLoader<TeamListItem[]>(
		(ws) => ws.listTeams().then((r) => r.items)
	);

	const columns = [
		{ key: 'name', label: 'Name', gridTrack: 'minmax(120px,1fr)' },
		{ key: 'summary', label: 'Summary', gridTrack: 'minmax(160px,2fr)' },
		{ key: 'member_count', label: 'Members', gridTrack: '100px' }
	] satisfies { key: keyof TeamListItem; label: string; gridTrack?: string }[];

	const rows = $derived(
		(teamLoader.data ?? []).map((t) => ({ ...t, summary: t.summary || '—' }))
	);

	let createOpen = $state(false);

	async function handleCreateTeam(name: string, summary: string): Promise<void> {
		const wsId = workspace.id;
		try {
			await api.forWorkspace(wsId).createTeam(name, summary || undefined);
			toastSuccess(`Team "${name}" created`);
			teamLoader.reload();
		} catch (e) {
			toastError(e instanceof ApiClientError ? e.body || e.message : String(e));
			throw e;
		}
	}
</script>

<svelte:head>
	<title>Teams | CoveFlow</title>
</svelte:head>

<PageFrame title="Teams" subtitle="Manage workspace teams">
	{#snippet actions()}
		<Button variant="primary" onclick={() => (createOpen = true)}>
			<Plus size={16} />
			New Team
		</Button>
	{/snippet}

	{#if teamLoader.error}
		<Alert variant="error">{teamLoader.error}</Alert>
	{:else}
		<div class="mt-6">
			<DataTable
				{columns}
				{rows}
				loading={teamLoader.loading}
				emptyText="No teams yet. Create one to get started."
				onRowClick={(row) => goto(`/admin/teams/${encodeURIComponent(row.name)}`)}
			/>
		</div>
	{/if}
</PageFrame>

<CreateTeamModal bind:open={createOpen} onConfirm={handleCreateTeam} />
