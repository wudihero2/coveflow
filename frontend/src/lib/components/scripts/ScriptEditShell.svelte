<script lang="ts">
	import { ArrowLeft, FileText } from '@lucide/svelte';
	import { tick, type Snippet } from 'svelte';

	import Button from '$lib/components/common/Button.svelte';
	import IconButton from '$lib/components/common/IconButton.svelte';
	import TextInput from '$lib/components/common/TextInput.svelte';

	interface Props {
		scriptPath: string;
		summary?: string;
		isDirty: boolean;
		canSave: boolean;
		saving: boolean;
		saveDisabledReason?: string;
		onBack: () => void;
		onSave: () => boolean | Promise<boolean>;
		children: Snippet;
		/** Optional Run button handler. When provided, a Run button is rendered next to Save. */
		onRun?: () => void | Promise<void>;
		/** Loading state for the Run button. */
		running?: boolean;
		/** Optional snippet rendered before the Run button (e.g. gear + popover). */
		runOptionsControl?: Snippet;
	}

	let {
		scriptPath,
		summary = $bindable(''),
		isDirty,
		canSave,
		saving,
		saveDisabledReason,
		onBack,
		onSave,
		children,
		onRun,
		running = false,
		runOptionsControl
	}: Props = $props();

	const SAVE_SUMMARY_ID = 'save-summary';

	let savePopoverOpen = $state(false);
	let popoverElement: HTMLDivElement | undefined = $state();

	async function openSavePopover() {
		if (!canSave) return;
		savePopoverOpen = true;
		await tick();
		document.getElementById(SAVE_SUMMARY_ID)?.focus();
	}

	async function submitSave() {
		if (!canSave) return;
		const saved = await onSave();
		if (saved) {
			savePopoverOpen = false;
		}
	}

	function handleSummaryKeydown(e: KeyboardEvent) {
		if (e.key === 'Enter') {
			e.preventDefault();
			void submitSave();
		}
	}

	$effect(() => {
		if (!savePopoverOpen) return;

		function handlePointerDown(event: PointerEvent) {
			const target = event.target;
			if (target instanceof Node && popoverElement?.contains(target)) return;
			savePopoverOpen = false;
		}

		function handleDocumentKeydown(event: KeyboardEvent) {
			if (event.key === 'Escape') {
				savePopoverOpen = false;
			}
		}

		document.addEventListener('pointerdown', handlePointerDown);
		document.addEventListener('keydown', handleDocumentKeydown);

		return () => {
			document.removeEventListener('pointerdown', handlePointerDown);
			document.removeEventListener('keydown', handleDocumentKeydown);
		};
	});
</script>

<div class="flex h-svh flex-col lg:h-svh max-lg:h-[calc(100svh-48px)]">
	<div class="flex items-center gap-2 border-b border-border px-3 py-2">
		<IconButton onclick={onBack} aria-label="Back to scripts">
			<ArrowLeft size={16} />
		</IconButton>

		<div class="flex items-center gap-1.5 text-sm">
			<FileText size={14} class="text-text-tertiary" />
			<span class="font-medium text-text">{scriptPath}</span>
		</div>

		<div class="flex-1"></div>

		{#if runOptionsControl}
			{@render runOptionsControl()}
		{/if}

		{#if onRun}
			<Button
				variant="primary"
				size="sm"
				onclick={() => void onRun?.()}
				loading={running}
				disabled={running}
			>
				Run
			</Button>
		{/if}

		<div class="relative">
			<Button
				variant={isDirty ? 'primary' : 'secondary'}
				size="sm"
				onclick={openSavePopover}
				loading={saving}
				disabled={!canSave}
				title={saveDisabledReason}
			>
				Save
			</Button>

			{#if savePopoverOpen}
				<div
					bind:this={popoverElement}
					class="absolute right-0 top-full z-50 mt-2 w-80 rounded-lg border border-border bg-surface-raised p-3 shadow-lg"
				>
					<TextInput
						id={SAVE_SUMMARY_ID}
						label="Summary"
						bind:value={summary}
						placeholder="What changed? (optional)"
						onkeydown={handleSummaryKeydown}
					/>
					<div class="mt-2 flex justify-end gap-2">
						<Button size="sm" variant="ghost" onclick={() => (savePopoverOpen = false)}>
							Cancel
						</Button>
						<Button
							size="sm"
							variant="primary"
							onclick={() => void submitSave()}
							loading={saving}
							disabled={!canSave}
						>
							Confirm
						</Button>
					</div>
				</div>
			{/if}
		</div>
	</div>

	{@render children()}
</div>
