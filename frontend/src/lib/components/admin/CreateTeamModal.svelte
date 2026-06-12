<script lang="ts">
	import Button from '$lib/components/common/Button.svelte';
	import Modal from '$lib/components/common/Modal.svelte';
	import TextInput from '$lib/components/common/TextInput.svelte';

	interface Props {
		open: boolean;
		onConfirm: (name: string, summary: string) => void | Promise<void>;
	}

	let { open = $bindable(false), onConfirm }: Props = $props();

	let name = $state('');
	let summary = $state('');
	let submitting = $state(false);
	let nameError = $state('');

	const NAME_RE = /^[a-z0-9][a-z0-9-]*$/;

	function validateName(v: string): string {
		if (!v) return '';
		if (v.length > 100) return 'Max 100 characters';
		if (!NAME_RE.test(v)) return 'Lowercase letters, numbers, and hyphens only';
		return '';
	}

	async function handleConfirm(): Promise<void> {
		nameError = validateName(name);
		if (nameError || !name) return;
		submitting = true;
		try {
			await onConfirm(name, summary);
			name = '';
			summary = '';
			open = false;
		} finally {
			submitting = false;
		}
	}

	$effect(() => {
		if (!open) {
			name = '';
			summary = '';
			nameError = '';
		}
	});
</script>

<Modal bind:open title="Create Team">
	<div class="space-y-4">
		<TextInput
			label="Team name"
			placeholder="e.g. ml-team"
			bind:value={name}
			error={nameError}
			oninput={() => (nameError = validateName(name))}
		/>
		<TextInput label="Summary (optional)" placeholder="Brief description" bind:value={summary} />
	</div>

	{#snippet actions()}
		<Button variant="secondary" onclick={() => (open = false)} disabled={submitting}>
			Cancel
		</Button>
		<Button variant="primary" onclick={handleConfirm} loading={submitting} disabled={!name || !!nameError}>
			Create team
		</Button>
	{/snippet}
</Modal>
