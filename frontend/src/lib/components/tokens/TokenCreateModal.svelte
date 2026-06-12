<script lang="ts">
	import { Check, Copy } from '@lucide/svelte';

	import Button from '$lib/components/common/Button.svelte';
	import Modal from '$lib/components/common/Modal.svelte';
	import SecretInput from '$lib/components/common/SecretInput.svelte';
	import TextInput from '$lib/components/common/TextInput.svelte';
	import { api, ApiClientError } from '$lib/services/api';
	import { toastError } from '$lib/toast';
	import type { ApiTokenCreated } from '$lib/types';

	interface Props {
		open: boolean;
		onClose: () => void;
		onCreated: () => void;
	}

	let { open = $bindable(false), onClose, onCreated }: Props = $props();

	let name = $state('');
	let expires = $state('');
	let creating = $state(false);
	let error = $state('');
	// Set after creation — the one-and-only time the plaintext is shown.
	let created = $state<ApiTokenCreated | null>(null);
	let copied = $state(false);

	$effect(() => {
		if (open) {
			name = '';
			expires = '';
			error = '';
			creating = false;
			created = null;
			copied = false;
		}
	});

	const canCreate = $derived(!creating && name.trim().length > 0);

	async function create(): Promise<void> {
		if (!canCreate) return;
		creating = true;
		error = '';
		try {
			const expiresAt = expires ? `${expires}T23:59:59Z` : undefined;
			created = await api.tokens.create(name.trim(), expiresAt);
		} catch (e) {
			error = e instanceof ApiClientError ? `${e.status}: ${e.body || e.message}` : String(e);
		} finally {
			creating = false;
		}
	}

	async function copy(): Promise<void> {
		if (!created) return;
		try {
			await navigator.clipboard.writeText(created.token);
			copied = true;
		} catch {
			toastError('Could not copy to clipboard');
		}
	}

	function done(): void {
		onCreated();
		onClose();
	}
</script>

<Modal {open} title="New API token">
	{#if created}
		<div class="flex flex-col gap-4">
			<p class="text-sm text-text-secondary">
				Copy your token now — this is the only time it's shown in full (you can reveal it again from
				the list, but keep it somewhere safe).
			</p>
			<div class="flex items-end gap-2">
				<div class="min-w-0 flex-1">
					<SecretInput value={created.token} label="Token" hint="" placeholder="" />
				</div>
				<Button variant="secondary" onclick={copy}>
					{#if copied}
						<Check size={15} class="text-success" /> Copied
					{:else}
						<Copy size={15} /> Copy
					{/if}
				</Button>
			</div>
		</div>
	{:else}
		<div class="flex flex-col gap-4">
			<TextInput bind:value={name} label="Name" placeholder="e.g. ci-deploy" mono />
			<TextInput bind:value={expires} label="Expires (optional)" type="date" />
			{#if error}
				<p class="text-sm text-error">{error}</p>
			{/if}
		</div>
	{/if}

	{#snippet actions()}
		{#if created}
			<Button variant="primary" onclick={done}>Done</Button>
		{:else}
			<Button variant="ghost" onclick={onClose}>Cancel</Button>
			<Button variant="primary" loading={creating} disabled={!canCreate} onclick={create}>
				Create
			</Button>
		{/if}
	{/snippet}
</Modal>
