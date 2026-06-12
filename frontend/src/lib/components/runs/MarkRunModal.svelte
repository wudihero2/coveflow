<script lang="ts">
	import { CircleCheck, CircleX, ShieldAlert } from '@lucide/svelte';
	import Button from '$lib/components/common/Button.svelte';
	import Modal from '$lib/components/common/Modal.svelte';
	import TextArea from '$lib/components/common/TextArea.svelte';
	import TextInput from '$lib/components/common/TextInput.svelte';

	type MarkVariant = 'success' | 'failure';

	interface Props {
		open: boolean;
		variant: MarkVariant;
		runId: string;
		onConfirm: (payload: { reason: string; result: unknown }) => void | Promise<void>;
		onCancel: () => void;
	}

	let { open = $bindable(false), variant, runId, onConfirm, onCancel }: Props = $props();

	let reason = $state('');
	let resultText = $state('');
	let submitting = $state(false);

	// Reset inputs each time the modal opens — otherwise switching between
	// Mark Success and Mark Fail would carry over the previous draft.
	$effect(() => {
		if (open) {
			reason = '';
			resultText = '';
			submitting = false;
		}
	});

	// Parse the JSON live so the Confirm button can stay disabled while the
	// text is invalid. Empty is allowed (sends `null`).
	const parsedResult = $derived.by<{ ok: boolean; value: unknown; error: string }>(() => {
		const trimmed = resultText.trim();
		if (trimmed === '') return { ok: true, value: null, error: '' };
		try {
			return { ok: true, value: JSON.parse(trimmed), error: '' };
		} catch (e) {
			return { ok: false, value: null, error: e instanceof Error ? e.message : 'Invalid JSON' };
		}
	});

	const parseError = $derived(parsedResult.error);

	async function handleConfirm(): Promise<void> {
		if (!parsedResult.ok) return;
		submitting = true;
		try {
			await onConfirm({ reason: reason.trim(), result: parsedResult.value });
		} finally {
			submitting = false;
		}
	}

	const variantConfig = {
		success: {
			title: 'Mark run as success?',
			icon: CircleCheck,
			iconColor: 'text-success',
			confirmLabel: 'Mark success',
			confirmVariant: 'primary' as const
		},
		failure: {
			title: 'Mark run as failure?',
			icon: CircleX,
			iconColor: 'text-error',
			confirmLabel: 'Mark failure',
			confirmVariant: 'danger' as const
		}
	};

	const config = $derived(variantConfig[variant]);
	const Icon = $derived(config.icon);
</script>

<Modal bind:open title={config.title}>
	<div class="space-y-4 text-sm">
		<div class="flex items-start gap-3 rounded-md border border-warning/30 bg-warning/5 p-3">
			<ShieldAlert size={18} class="mt-0.5 shrink-0 text-warning" />
			<p class="text-text-secondary">
				This admin override writes a terminal state on the run regardless of its current status.
				Use it only to repair stuck or misreported jobs.
			</p>
		</div>

		<div class="flex items-center gap-2 text-text">
			<Icon size={16} class={config.iconColor} />
			<span>
				Marking <code class="rounded bg-surface-alt px-1 font-mono">{runId.slice(0, 8)}</code>
			</span>
		</div>

		<TextInput
			id="mark-reason"
			label="Reason"
			bind:value={reason}
			placeholder="What happened? (visible in audit log)"
			disabled={submitting}
		/>

		<div>
			<TextArea
				id="mark-result"
				label="Result (JSON, optional)"
				bind:value={resultText}
				placeholder={'{"note": "manually resolved"}'}
				mono
				disabled={submitting}
			/>
			{#if parseError}
				<p class="mt-1 text-xs text-error">Invalid JSON: {parseError}</p>
			{/if}
		</div>
	</div>

	{#snippet actions()}
		<Button variant="secondary" size="sm" onclick={onCancel} disabled={submitting}>
			Cancel
		</Button>
		<Button
			variant={config.confirmVariant}
			size="sm"
			onclick={() => void handleConfirm()}
			loading={submitting}
			disabled={!parsedResult.ok}
		>
			{config.confirmLabel}
		</Button>
	{/snippet}
</Modal>
