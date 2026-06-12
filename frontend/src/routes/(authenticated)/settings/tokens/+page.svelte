<script lang="ts">
	import { Copy, Eye, KeyRound, Trash2 } from '@lucide/svelte';

	import Alert from '$lib/components/common/Alert.svelte';
	import Button from '$lib/components/common/Button.svelte';
	import DataTable from '$lib/components/common/DataTable.svelte';
	import IconButton from '$lib/components/common/IconButton.svelte';
	import Modal from '$lib/components/common/Modal.svelte';
	import PageFrame from '$lib/components/common/PageFrame.svelte';
	import SecretInput from '$lib/components/common/SecretInput.svelte';
	import TokenCreateModal from '$lib/components/tokens/TokenCreateModal.svelte';
	import { api, ApiClientError } from '$lib/services/api';
	import { confirmDialog } from '$lib/stores/confirm.svelte';
	import { toastError, toastSuccess } from '$lib/toast';
	import type { ApiTokenListItem } from '$lib/types';
	import { formatRelative } from '$lib/utils/format-time';

	let tokens = $state.raw<ApiTokenListItem[]>([]);
	let loading = $state(true);
	let loadError = $state('');

	let createOpen = $state(false);
	let revealed = $state<{ name: string; token: string } | null>(null);

	$effect(() => {
		void load();
	});

	async function load(): Promise<void> {
		loading = true;
		loadError = '';
		try {
			tokens = await api.tokens.list();
		} catch (e) {
			loadError = e instanceof ApiClientError ? e.message : 'Failed to load tokens';
		} finally {
			loading = false;
		}
	}

	// DataTable keys must be real fields → carry a synthetic `actions` column.
	type Row = ApiTokenListItem & { actions: string };
	const rows = $derived<Row[]>(tokens.map((t) => ({ ...t, actions: '' })));
	const columns: { key: keyof Row & string; label: string; gridTrack?: string }[] = [
		{ key: 'name', label: 'Name', gridTrack: 'minmax(0,2fr)' },
		{ key: 'created_at', label: 'Created', gridTrack: '120px' },
		{ key: 'last_used_at', label: 'Last used', gridTrack: '120px' },
		{ key: 'expires_at', label: 'Expires', gridTrack: '120px' },
		{ key: 'actions', label: '', gridTrack: '88px' }
	];

	async function reveal(t: ApiTokenListItem): Promise<void> {
		try {
			const { token } = await api.tokens.reveal(t.id);
			revealed = { name: t.name, token };
		} catch {
			toastError('Could not reveal token');
		}
	}

	async function copyRevealed(): Promise<void> {
		if (!revealed) return;
		try {
			await navigator.clipboard.writeText(revealed.token);
			toastSuccess('Copied');
		} catch {
			toastError('Could not copy to clipboard');
		}
	}

	async function revoke(t: ApiTokenListItem): Promise<void> {
		const ok = await confirmDialog({
			title: `Revoke "${t.name}"?`,
			message: 'Any webhook or integration using this token will stop working immediately.',
			confirmLabel: 'Revoke',
			variant: 'danger'
		});
		if (!ok) return;
		try {
			await api.tokens.revoke(t.id);
			toastSuccess('Revoked');
			void load();
		} catch {
			toastError('Could not revoke token');
		}
	}
</script>

<PageFrame title="API Tokens" subtitle="Personal tokens for calling webhooks as you. Keep them secret.">
	{#snippet actions()}
		<Button variant="primary" onclick={() => (createOpen = true)}>
			<KeyRound size={16} /> New token
		</Button>
	{/snippet}

	{#if loadError}
		<Alert variant="error">{loadError}</Alert>
	{:else}
		<DataTable {rows} {columns} {loading} emptyText="No tokens yet">
			{#snippet cell({ row, column })}
				{#if column.key === 'name'}
					<span class="truncate font-medium text-text" title={row.name}>{row.name}</span>
				{:else if column.key === 'created_at'}
					<time class="text-xs text-text-tertiary" title={row.created_at}>
						{formatRelative(row.created_at)}
					</time>
				{:else if column.key === 'last_used_at'}
					<span class="text-xs text-text-tertiary">
						{row.last_used_at ? formatRelative(row.last_used_at) : 'never'}
					</span>
				{:else if column.key === 'expires_at'}
					<span class="text-xs text-text-tertiary">
						{row.expires_at ? formatRelative(row.expires_at) : 'never'}
					</span>
				{:else if column.key === 'actions'}
					<div class="flex items-center justify-end gap-1">
						<IconButton aria-label="Reveal" onclick={() => reveal(row)}><Eye size={15} /></IconButton>
						<IconButton aria-label="Revoke" onclick={() => revoke(row)}>
							<Trash2 size={15} class="text-error" />
						</IconButton>
					</div>
				{/if}
			{/snippet}
		</DataTable>
	{/if}
</PageFrame>

<TokenCreateModal bind:open={createOpen} onClose={() => (createOpen = false)} onCreated={load} />

<Modal
	open={revealed !== null}
	title={revealed ? `Token · ${revealed.name}` : 'Token'}
	size="lg"
>
	{#if revealed}
		<div class="flex items-end gap-2">
			<div class="min-w-0 flex-1">
				<SecretInput value={revealed.token} hint="" placeholder="" />
			</div>
			<Button variant="secondary" onclick={copyRevealed}><Copy size={15} /> Copy</Button>
		</div>
	{/if}
	{#snippet actions()}
		<Button variant="primary" onclick={() => (revealed = null)}>Close</Button>
	{/snippet}
</Modal>
