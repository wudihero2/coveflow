<script lang="ts">
	import { TriangleAlert } from '@lucide/svelte';
	import Button from '$lib/components/common/Button.svelte';
	import Modal from '$lib/components/common/Modal.svelte';
	import TextInput from '$lib/components/common/TextInput.svelte';

	interface Props {
		open: boolean;
		runId: string;
		/** Receives the (possibly empty) reason string from the input. */
		onConfirm: (reason: string) => void | Promise<void>;
		onCancel: () => void;
	}

	let { open = $bindable(false), runId, onConfirm, onCancel }: Props = $props();

	let reason = $state('');
	let submitting = $state(false);

	// Reset the input every time the modal re-opens so a previous Cancel
	// attempt doesn't leak its reason into the next one.
	$effect(() => {
		if (open) {
			reason = '';
			submitting = false;
		}
	});

	async function handleConfirm(): Promise<void> {
		submitting = true;
		try {
			await onConfirm(reason.trim());
		} finally {
			submitting = false;
		}
	}
</script>

<Modal bind:open title="Cancel run?">
	<div class="flex gap-3">
		<TriangleAlert size={20} class="mt-0.5 shrink-0 text-warning" />
		<div class="flex-1 space-y-3 text-sm">
			<p class="text-text">
				Run
				<code class="rounded bg-surface-alt px-1 font-mono text-text">{runId.slice(0, 8)}</code>
				will be terminated. Logs collected so far remain available.
			</p>
			<TextInput
				id="cancel-reason"
				label="Reason (optional)"
				bind:value={reason}
				placeholder="e.g. wrong inputs, stuck"
				disabled={submitting}
			/>
		</div>
	</div>

	{#snippet actions()}
		<Button variant="secondary" size="sm" onclick={onCancel} disabled={submitting}>
			Keep running
		</Button>
		<Button variant="danger" size="sm" onclick={() => void handleConfirm()} loading={submitting}>
			Cancel run
		</Button>
	{/snippet}
</Modal>
