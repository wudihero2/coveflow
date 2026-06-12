<script lang="ts">
	import { TriangleAlert } from '@lucide/svelte';
	import Button from '$lib/components/common/Button.svelte';
	import Modal from '$lib/components/common/Modal.svelte';

	interface Props {
		open: boolean;
		/** Short hash (8 chars) of the version about to be restored, shown in the warning copy. */
		versionHash: string;
		onConfirm: () => void;
		onCancel: () => void;
	}

	let { open = $bindable(false), versionHash, onConfirm, onCancel }: Props = $props();
</script>

<Modal bind:open title="Discard unsaved changes?">
	<div class="flex gap-3">
		<TriangleAlert size={20} class="mt-0.5 shrink-0 text-warning" />
		<div class="space-y-1 text-sm">
			<p class="text-text">
				Restoring
				<code class="rounded bg-surface-alt px-1 font-mono text-text">{versionHash.slice(0, 8)}</code>
				will discard your unsaved edits.
			</p>
			<p class="text-text-secondary">You can save afterwards to create a new version.</p>
		</div>
	</div>

	{#snippet actions()}
		<Button variant="secondary" size="sm" onclick={onCancel}>Cancel</Button>
		<Button variant="danger" size="sm" onclick={onConfirm}>Discard &amp; Restore</Button>
	{/snippet}
</Modal>
