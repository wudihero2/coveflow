<script lang="ts">
	import type { Snippet } from 'svelte';
	import Button from '$lib/components/common/Button.svelte';
	import Modal from '$lib/components/common/Modal.svelte';

	interface Props {
		open: boolean;
		title: string;
		message?: string;
		body?: Snippet;
		confirmLabel?: string;
		variant?: 'danger' | 'primary';
		onConfirm: () => void | Promise<void>;
	}

	let {
		open = $bindable(false),
		title,
		message,
		body,
		confirmLabel = 'Confirm',
		variant = 'danger',
		onConfirm
	}: Props = $props();

	let confirming = $state(false);

	async function handleConfirm(): Promise<void> {
		confirming = true;
		try {
			await onConfirm();
			open = false;
		} finally {
			confirming = false;
		}
	}
</script>

<Modal bind:open {title}>
	{#if body}
		{@render body()}
	{:else if message}
		<p class="text-sm text-text-secondary">{message}</p>
	{/if}

	{#snippet actions()}
		<Button variant="secondary" onclick={() => (open = false)} disabled={confirming}>
			Cancel
		</Button>
		<Button {variant} onclick={handleConfirm} loading={confirming}>
			{confirmLabel}
		</Button>
	{/snippet}
</Modal>
