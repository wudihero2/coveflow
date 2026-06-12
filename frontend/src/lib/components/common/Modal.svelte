<script lang="ts">
	import type { Snippet } from 'svelte';

	interface Props {
		open: boolean;
		title: string;
		children: Snippet;
		actions?: Snippet;
		/** Max width. Defaults to `md` to preserve existing dialogs. */
		size?: 'md' | 'lg' | 'xl';
	}

	let { open = $bindable(false), title, children, actions, size = 'md' }: Props = $props();
	let dialog: HTMLDialogElement;

	const maxWidth = $derived({ md: 'max-w-md', lg: 'max-w-lg', xl: 'max-w-2xl' }[size]);

	const generatedId = $props.id();
	const titleId = `${generatedId}-title`;

	$effect(() => {
		if (open && !dialog.open) dialog.showModal();
		else if (!open && dialog.open) dialog.close();
	});

	function onBackdropClick(e: MouseEvent) {
		if (e.target === dialog) open = false;
	}
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<dialog
	bind:this={dialog}
	aria-labelledby={titleId}
	class="m-auto w-[calc(100vw-2rem)] {maxWidth} rounded-lg border border-border bg-surface-raised p-6 shadow-lg backdrop:bg-black/40"
	onclick={onBackdropClick}
	onclose={() => (open = false)}
>
	<h3 id={titleId} class="text-lg font-semibold text-text">{title}</h3>
	<div class="py-4">{@render children()}</div>
	{#if actions}
		<div class="flex justify-end gap-2">{@render actions()}</div>
	{/if}
</dialog>
