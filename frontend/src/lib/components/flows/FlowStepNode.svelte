<script lang="ts">
	import { Handle, Position } from '@xyflow/svelte';
	import {
		Code,
		RotateCw,
		GitBranch,
		Trash2,
		Settings2,
		TriangleAlert,
		type Icon as IconType
	} from '@lucide/svelte';

	import { STEP_NODE_W, type StepNodeData, type StepStatus } from '$lib/services/flow-graph';
	import type { NodeBody } from '$lib/types';

	let { data }: { data: StepNodeData } = $props();

	function iconFor(kind: NodeBody['kind']): typeof IconType {
		switch (kind) {
			case 'script':
				return Code;
			case 'branch':
				return GitBranch;
		}
	}

	// Status → pill. `idle` (design-time, no run) shows no pill.
	function pillFor(status: StepStatus): { label: string; cls: string } | null {
		switch (status) {
			case 'running':
				return { label: 'running', cls: 'bg-accent/15 text-accent' };
			case 'succeeded':
				return { label: 'success', cls: 'bg-success/15 text-success' };
			case 'failed':
				return { label: 'failed', cls: 'bg-error/15 text-error' };
			case 'skipped':
				return { label: 'skipped', cls: 'bg-surface-alt text-text-tertiary' };
			case 'pending':
				return { label: 'pending', cls: 'bg-surface-alt text-text-tertiary' };
			default:
				return null;
		}
	}

	function accentFor(status: StepStatus): string {
		switch (status) {
			case 'running':
				return 'border-l-accent';
			case 'succeeded':
				return 'border-l-success';
			case 'failed':
				return 'border-l-error';
			case 'skipped':
				return 'border-l-text-tertiary';
			default:
				// Idle: brand accent (Cove blue) so cards read clearly against the canvas.
				return 'border-l-accent';
		}
	}

	const Icon = $derived(iconFor(data.node.body.kind));
	const pill = $derived(pillFor(data.status));
	// Number of retries configured on this node (0 = none), shown as a small badge.
	const retries = $derived(data.node.retry?.max_attempts ?? 0);
	// Broken (referenced script deleted) wins over status for the accent so it
	// stands out even at idle in the editor.
	const accent = $derived(data.broken ? 'border-l-error' : accentFor(data.status));
	const interactive = $derived(!!data.onSelect);
	const handleCls = $derived(interactive ? '!h-2.5 !w-2.5 !bg-border-strong' : '!opacity-0');
</script>

<Handle type="target" position={Position.Top} id="t" class={handleCls} />
<Handle type="target" position={Position.Left} id="l" class={handleCls} />

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
	class="group relative flex flex-col gap-0.5 rounded-lg border border-l-4 bg-surface-raised px-3 py-2 shadow-sm transition
		{accent} {data.selected ? 'border-accent ring-1 ring-accent' : 'border-border'}
		{interactive ? 'cursor-pointer hover:shadow-md' : ''}"
	style="width:{STEP_NODE_W}px"
	onclick={() => data.onSelect?.(data.node.id)}
	onkeydown={(e) => {
		if (e.key === 'Enter' || e.key === ' ') {
			e.preventDefault();
			data.onSelect?.(data.node.id);
		}
	}}
>
	<!-- Single line: the script reference (e.g. "script · a/a0"). The node id is
	     an internal reference handle, shown only in the settings panel. -->
	<div class="flex items-center gap-2">
		<Icon size={15} class="shrink-0 {data.broken ? 'text-error' : 'text-accent'}" />
		<span class="min-w-0 flex-1 truncate text-sm text-text-secondary">{data.subtitle}</span>
		{#if retries > 0}
			<span
				class="inline-flex shrink-0 items-center gap-0.5 rounded-full bg-surface-alt px-1.5 py-0.5 text-[10px] font-medium text-text-tertiary"
				title="Retries on failure: {retries}"
			>
				<RotateCw size={9} />
				{retries}
			</span>
		{/if}
		{#if data.broken}
			<span
				class="inline-flex shrink-0 items-center gap-1 rounded-full bg-error/15 px-2 py-0.5 text-[10px] font-medium text-error"
				title="The referenced script was deleted or moved. Reassign or remove this node."
			>
				<TriangleAlert size={10} />
				missing script
			</span>
		{:else if pill}
			<span class="shrink-0 rounded-full px-2 py-0.5 text-[10px] font-medium {pill.cls}">
				{pill.label}
			</span>
		{/if}
	</div>

	{#if data.onDelete}
		<div class="absolute -top-2.5 right-2 flex gap-1 opacity-0 transition group-hover:opacity-100">
			<button
				type="button"
				title="Configure node"
				class="flex h-5 w-5 items-center justify-center rounded-full border border-border bg-surface-raised text-text-secondary shadow-sm hover:text-accent"
				onclick={(e) => {
					e.stopPropagation();
					data.onSelect?.(data.node.id);
				}}
			>
				<Settings2 size={11} />
			</button>
			<button
				type="button"
				title="Delete node"
				class="flex h-5 w-5 items-center justify-center rounded-full border border-border bg-surface-raised text-text-secondary shadow-sm hover:text-error"
				onclick={(e) => {
					e.stopPropagation();
					data.onDelete?.(data.node.id);
				}}
			>
				<Trash2 size={11} />
			</button>
		</div>
	{/if}
</div>

<Handle type="source" position={Position.Bottom} id="b" class={handleCls} />
<Handle type="source" position={Position.Right} id="r" class={handleCls} />
