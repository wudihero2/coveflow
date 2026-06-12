<script lang="ts">
	import Button from '$lib/components/common/Button.svelte';
	import Modal from '$lib/components/common/Modal.svelte';
	import Select from '$lib/components/common/Select.svelte';
	import SecretInput from '$lib/components/common/SecretInput.svelte';
	import TextInput from '$lib/components/common/TextInput.svelte';
	import { api, ApiClientError } from '$lib/services/api';
	import { toastSuccess } from '$lib/toast';

	interface Props {
		open: boolean;
		/** 'create' shows root + name pickers; 'rotate' locks the path. */
		mode: 'create' | 'rotate';
		workspaceId: string;
		/** Writable three-root prefixes for create, e.g. `workspace/`, `users/me/`. */
		roots?: string[];
		/** The path being rotated (mode='rotate'). */
		path?: string;
		onSaved: () => void;
	}

	let {
		open = $bindable(false),
		mode,
		workspaceId,
		roots = [],
		path = '',
		onSaved
	}: Props = $props();

	let root = $state('');
	let name = $state('');
	let value = $state('');
	let description = $state('');
	let saving = $state(false);
	let error = $state('');

	// Reset the form whenever the modal opens (the parent reuses one instance).
	$effect(() => {
		if (open) {
			root = roots[0] ?? '';
			name = '';
			value = '';
			description = '';
			error = '';
			saving = false;
		}
	});

	const rootOptions = $derived(roots.map((r) => ({ label: r, value: r })));
	const fullPath = $derived(mode === 'rotate' ? path : `${root}${name.trim()}`);
	const canSave = $derived(
		!saving && value.length > 0 && (mode === 'rotate' || name.trim().length > 0)
	);

	async function save(): Promise<void> {
		if (!canSave) return;
		saving = true;
		error = '';
		const ws = api.forWorkspace(workspaceId);
		const body = { value, description: description.trim() || undefined };
		try {
			if (mode === 'create') {
				await ws.createSecret({ path: fullPath, ...body });
				toastSuccess(`Secret "${fullPath}" created`);
			} else {
				await ws.rotateSecret(path, body);
				toastSuccess(`Secret "${path}" rotated`);
			}
			open = false;
			onSaved();
		} catch (e) {
			error = e instanceof ApiClientError ? e.message : 'Failed to save secret';
		} finally {
			saving = false;
		}
	}
</script>

<Modal bind:open title={mode === 'create' ? 'New secret' : 'Rotate secret'}>
	<div class="flex flex-col gap-4">
		{#if mode === 'create'}
			<div>
				<span class="block text-sm font-medium text-text-secondary">Location</span>
				<div class="mt-2 flex items-center gap-2">
					<div class="w-44 shrink-0">
						<Select ariaLabel="Root" options={rootOptions} bind:value={root} />
					</div>
					<TextInput bind:value={name} placeholder="name e.g. openai_api_key" mono />
				</div>
				<p class="mt-1 break-all text-xs text-text-tertiary">
					Full key: <code>{fullPath || '—'}</code>
				</p>
			</div>
		{:else}
			<div>
				<span class="block text-sm font-medium text-text-secondary">Key</span>
				<p class="mt-2 break-all font-mono text-sm text-text">{path}</p>
			</div>
		{/if}

		<SecretInput bind:value label="Value" />

		<TextInput bind:value={description} label="Description (optional)" />

		{#if error}
			<p class="text-sm text-error">{error}</p>
		{/if}
	</div>

	{#snippet actions()}
		<Button variant="ghost" onclick={() => (open = false)}>Cancel</Button>
		<Button variant="primary" loading={saving} disabled={!canSave} onclick={save}>
			{mode === 'create' ? 'Create' : 'Rotate'}
		</Button>
	{/snippet}
</Modal>
