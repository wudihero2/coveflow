<script lang="ts">
	import Button from '$lib/components/common/Button.svelte';
	import type { ScriptDraft } from '$lib/services/script-drafts';
	import { formatAbsolute } from '$lib/utils/format-time';
	import { displayTz } from '$lib/stores/timezone.svelte';

	interface Props {
		draft: ScriptDraft | null;
		isStale?: boolean;
		onRestore: () => void;
		onDiscard: () => void;
	}

	let { draft, isStale = false, onRestore, onDiscard }: Props = $props();

	let savedAt = $derived(draft ? formatAbsolute(draft.updatedAt, displayTz.value) : '');
</script>

{#if draft}
	<div class="border-b border-accent/20 bg-accent/5 px-3 py-2">
		<div class="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
			<div class="min-w-0">
				<p class="text-sm font-medium text-text">Local draft available</p>
				<p class="text-xs text-text-tertiary">
					Saved {savedAt}. {isStale
						? 'It was based on an older script version.'
						: 'Restore it to continue editing.'}
				</p>
			</div>

			<div class="flex shrink-0 items-center gap-2">
				<Button size="sm" variant="ghost" onclick={onDiscard}>Discard</Button>
				<Button size="sm" variant="primary" onclick={onRestore}>Restore</Button>
			</div>
		</div>
	</div>
{/if}
