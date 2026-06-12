<script lang="ts">
	import { Plus, GitBranch, Search, Code } from '@lucide/svelte';

	import type { ScriptListItem } from '$lib/types';

	interface Props {
		scripts: ScriptListItem[];
		/** Add the picked script as a node; `asBranch` wraps it as a Branch operator. */
		onAdd: (script: ScriptListItem, asBranch: boolean) => void;
	}

	let { scripts, onAdd }: Props = $props();

	let open = $state(false);
	let query = $state('');
	let asBranch = $state(false);
	let container = $state<HTMLDivElement>();
	let input = $state<HTMLInputElement>();

	// Search-only: the left file tree is the browser, so show results only once
	// the user types (empty query → no list).
	const filtered = $derived.by(() => {
		const q = query.trim().toLowerCase();
		if (!q) return [];
		return scripts
			.filter((s) => s.path.toLowerCase().includes(q) || s.name.toLowerCase().includes(q))
			.slice(0, 50);
	});

	function toggle(): void {
		open = !open;
		if (open) {
			query = '';
			// Focus the search box once the popover is in the DOM.
			queueMicrotask(() => input?.focus());
		}
	}

	function pick(s: ScriptListItem): void {
		onAdd(s, asBranch);
		// Stay open so several nodes can be added in a row.
	}

	function leaf(path: string): string {
		return path.split('/').pop() || path;
	}
</script>

<svelte:window
	onclick={(e) => {
		if (open && container && !container.contains(e.target as Node)) open = false;
	}}
	onkeydown={(e) => {
		if (open && e.key === 'Escape') open = false;
	}}
/>

<div bind:this={container} class="relative">
	<button
		type="button"
		class="inline-flex items-center gap-1.5 rounded-md border border-border bg-surface-raised px-2.5 py-1.5 text-xs font-medium text-text shadow-sm transition-colors hover:bg-surface-alt"
		aria-expanded={open}
		onclick={toggle}
	>
		<Plus size={14} /> Add node
	</button>

	{#if open}
		<div
			class="absolute top-full left-0 z-20 mt-1 flex max-h-[60vh] w-72 flex-col overflow-hidden rounded-lg border border-border bg-surface-raised shadow-lg"
			role="dialog"
			aria-label="Add node"
		>
			<div class="flex items-center gap-2 border-b border-border px-2.5 py-2">
				<Search size={14} class="shrink-0 text-text-tertiary" />
				<input
					bind:this={input}
					bind:value={query}
					type="text"
					placeholder="Search scripts…"
					class="min-w-0 flex-1 bg-transparent text-sm text-text outline-none placeholder:text-text-tertiary"
				/>
			</div>

			<label
				class="flex cursor-pointer items-center gap-2 border-b border-border-subtle px-2.5 py-2 text-xs whitespace-nowrap text-text-secondary"
			>
				<input type="checkbox" bind:checked={asBranch} class="size-3.5 shrink-0 accent-accent" />
				<GitBranch size={13} class="shrink-0 text-text-tertiary" /> Add as Branch node
			</label>

			<div class="min-h-0 flex-1 overflow-y-auto py-1">
				{#if query.trim() === ''}
					<p class="px-3 py-4 text-center text-xs text-text-tertiary">
						Type to search scripts.
					</p>
				{:else if filtered.length === 0}
					<p class="px-3 py-4 text-center text-xs text-text-tertiary">No match.</p>
				{:else}
					{#each filtered as s (s.script_id)}
						<button
							type="button"
							class="flex w-full items-center gap-2 px-2.5 py-1.5 text-left hover:bg-surface-alt"
							onclick={() => pick(s)}
						>
							<Code size={14} class="shrink-0 text-accent" />
							<span class="flex min-w-0 flex-col">
								<span class="truncate text-sm text-text">{leaf(s.path)}</span>
								<span class="truncate font-mono text-[11px] text-text-tertiary">{s.path}</span>
							</span>
						</button>
					{/each}
				{/if}
			</div>
		</div>
	{/if}
</div>
