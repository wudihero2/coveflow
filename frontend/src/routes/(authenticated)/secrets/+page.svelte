<script lang="ts">
	import { KeyRound } from '@lucide/svelte';

	import Alert from '$lib/components/common/Alert.svelte';
	import Button from '$lib/components/common/Button.svelte';
	import PageFrame from '$lib/components/common/PageFrame.svelte';
	import SecretFormModal from '$lib/components/secrets/SecretFormModal.svelte';
	import SecretTable from '$lib/components/secrets/SecretTable.svelte';
	import { api } from '$lib/services/api';
	import { auth } from '$lib/stores/auth.svelte';
	import { confirmDialog } from '$lib/stores/confirm.svelte';
	import { workspace } from '$lib/stores/workspace.svelte';
	import { toastError, toastSuccess } from '$lib/toast';
	import type { SecretListItem } from '$lib/types';

	let secrets = $state.raw<SecretListItem[]>([]);
	let roots = $state.raw<string[]>([]);
	let loading = $state(true);
	let loadError = $state('');
	let generation = 0;

	// Modal state: create (no path) or rotate (locked path).
	let modalOpen = $state(false);
	let modalMode = $state<'create' | 'rotate'>('create');
	let rotatePath = $state('');

	// Reload whenever the workspace changes; guard against stale responses.
	$effect(() => {
		const wsId = workspace.id;
		if (!wsId) return;
		void load(wsId);
	});

	async function load(wsId: string): Promise<void> {
		const gen = ++generation;
		loading = true;
		loadError = '';
		try {
			const ws = api.forWorkspace(wsId);
			const [list, me] = await Promise.all([ws.listSecrets(), ws.getMe()]);
			if (gen !== generation || workspace.id !== wsId) return;
			secrets = list;
			roots = buildRoots(me.role, me.writable_teams);
		} catch {
			if (gen !== generation) return;
			loadError = 'Failed to load secrets';
		} finally {
			if (gen === generation) loading = false;
		}
	}

	// The three-root prefixes the user may create under: their own user root,
	// each writable team, and the workspace share (unless a viewer).
	function buildRoots(role: string, writableTeams: string[]): string[] {
		const out = [`users/${auth.email}/`];
		for (const t of writableTeams) out.push(`teams/${t}/`);
		if (role !== 'viewer') out.push('workspace/');
		return out;
	}

	function openCreate(): void {
		modalMode = 'create';
		rotatePath = '';
		modalOpen = true;
	}

	function openRotate(path: string): void {
		modalMode = 'rotate';
		rotatePath = path;
		modalOpen = true;
	}

	async function remove(item: SecretListItem): Promise<void> {
		const ok = await confirmDialog({
			title: 'Delete secret',
			message: `Delete "${item.path}"? Scripts using Secret.get("${item.path}") will stop receiving it.`,
			confirmLabel: 'Delete',
			variant: 'danger'
		});
		if (!ok) return;
		try {
			await api.forWorkspace(workspace.id).deleteSecret(item.path);
			toastSuccess(`Secret "${item.path}" deleted`);
			void load(workspace.id);
		} catch {
			toastError('Failed to delete secret');
		}
	}
</script>

<PageFrame title="Secrets" subtitle="Encrypted, write-only values your scripts read with Secret.get()">
	{#snippet actions()}
		<Button variant="primary" onclick={openCreate} disabled={loading || roots.length === 0}>
			<KeyRound size={16} />
			New secret
		</Button>
	{/snippet}

	{#if loadError}
		<Alert variant="error">{loadError}</Alert>
	{:else}
		<SecretTable {secrets} {loading} onRotate={openRotate} onDelete={remove} />
	{/if}
</PageFrame>

<SecretFormModal
	bind:open={modalOpen}
	mode={modalMode}
	workspaceId={workspace.id}
	{roots}
	path={rotatePath}
	onSaved={() => load(workspace.id)}
/>
