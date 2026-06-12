<script lang="ts">
	import { RotateCw } from '@lucide/svelte';
	import Button from '$lib/components/common/Button.svelte';
	import Checkbox from '$lib/components/common/Checkbox.svelte';
	import Modal from '$lib/components/common/Modal.svelte';

	interface Props {
		open: boolean;
		runId: string;
		onConfirm: (useLatestVersion: boolean) => void | Promise<void>;
		onCancel: () => void;
	}

	let { open = $bindable(false), runId, onConfirm, onCancel }: Props = $props();

	let useLatest = $state(false);
	let submitting = $state(false);

	// Default the checkbox back to its safer state (same hash) each time the
	// modal opens so a previous "use latest" choice doesn't stick.
	$effect(() => {
		if (open) {
			useLatest = false;
			submitting = false;
		}
	});

	async function handleConfirm(): Promise<void> {
		submitting = true;
		try {
			await onConfirm(useLatest);
		} finally {
			submitting = false;
		}
	}
</script>

<Modal bind:open title="Rerun this script?">
	<div class="flex gap-3">
		<RotateCw size={20} class="mt-0.5 shrink-0 text-info" />
		<div class="flex-1 space-y-3 text-sm">
			<p class="text-text">
				A new run will be created from
				<code class="rounded bg-surface-alt px-1 font-mono text-text">{runId.slice(0, 8)}</code>.
			</p>
			<Checkbox
				bind:checked={useLatest}
				label="Use latest saved version"
				description="Resolve the script path to its newest hash. Off keeps the exact original code."
				disabled={submitting}
			/>
		</div>
	</div>

	{#snippet actions()}
		<Button variant="secondary" size="sm" onclick={onCancel} disabled={submitting}>
			Cancel
		</Button>
		<Button variant="primary" size="sm" onclick={() => void handleConfirm()} loading={submitting}>
			Rerun
		</Button>
	{/snippet}
</Modal>
