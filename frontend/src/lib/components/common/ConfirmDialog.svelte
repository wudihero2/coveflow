<script lang="ts">
	import Button from '$lib/components/common/Button.svelte';
	import Modal from '$lib/components/common/Modal.svelte';
	import { confirmStore } from '$lib/stores/confirm.svelte';

	// Modal owns the <dialog> and flips `open` to false on backdrop click / Escape.
	// Derive it from the store; Modal may still override it to false on dismiss,
	// which the effect below treats as a cancel of the pending request.
	let open = $derived(confirmStore.current !== null);
	$effect(() => {
		if (!open && confirmStore.current) confirmStore.settle(false);
	});
</script>

{#if confirmStore.current}
	{@const c = confirmStore.current}
	<Modal bind:open title={c.title}>
		{#if c.message}
			<p class="text-sm whitespace-pre-line text-text-secondary">{c.message}</p>
		{/if}

		{#snippet actions()}
			<Button variant="secondary" onclick={() => confirmStore.settle(false)}>
				{c.cancelLabel ?? 'Cancel'}
			</Button>
			<Button variant={c.variant ?? 'danger'} onclick={() => confirmStore.settle(true)}>
				{c.confirmLabel ?? 'Confirm'}
			</Button>
		{/snippet}
	</Modal>
{/if}
