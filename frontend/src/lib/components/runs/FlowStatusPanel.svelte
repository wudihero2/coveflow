<script lang="ts">
	import { ExternalLink } from '@lucide/svelte';
	import { goto } from '$app/navigation';

	import type { FlowRunState, NodeRunState } from '$lib/types';

	interface Props {
		status: FlowRunState;
	}

	let { status }: Props = $props();

	// Map each node state to a label + dot colour.
	function tone(s: NodeRunState): { label: string; dot: string; text: string } {
		switch (s.state) {
			case 'succeeded':
				return { label: 'succeeded', dot: 'bg-success', text: 'text-text' };
			case 'failed':
				return { label: 'failed', dot: 'bg-error', text: 'text-error' };
			case 'running':
				return { label: 'running', dot: 'bg-accent animate-pulse', text: 'text-text' };
			case 'skipped':
				return { label: 'skipped', dot: 'bg-text-tertiary', text: 'text-text-tertiary' };
			default:
				return { label: 'pending', dot: 'bg-border', text: 'text-text-tertiary' };
		}
	}

	function fanoutLabel(s: NodeRunState): string | null {
		if (s.state === 'running' && s.fanout) {
			return `${s.fanout.completed}/${s.fanout.total}`;
		}
		return null;
	}

	// A child run exists for nodes that dispatched one (running/succeeded/failed).
	// A node that failed *before* dispatch (e.g. its script was deleted) has none.
	function runIdOf(s: NodeRunState): string | undefined {
		return 'run_id' in s ? s.run_id : undefined;
	}

	// Pull a readable message out of the engine's failure payload
	// (`{ error: { message } }`), falling back to a compact JSON dump.
	function failureText(s: NodeRunState): string | null {
		if (s.state !== 'failed') return null;
		const e = s.error;
		if (typeof e === 'string') return e;
		if (e && typeof e === 'object') {
			const obj = e as Record<string, unknown>;
			const inner = (obj.error ?? obj) as Record<string, unknown>;
			if (typeof inner.message === 'string') return inner.message;
		}
		try {
			return JSON.stringify(e);
		} catch {
			return 'unknown error';
		}
	}
</script>

{#snippet row(label: string, labelCls: string, t: ReturnType<typeof tone>, fanout: string | null, rid: string | undefined)}
	<!-- Real button so it's keyboard-accessible; disabled (non-clickable) when the
	     node dispatched no child run to open. -->
	<button
		type="button"
		disabled={!rid}
		class="flex items-center gap-2 text-left text-sm {rid
			? 'cursor-pointer hover:text-accent'
			: 'cursor-default'}"
		onclick={() => {
			if (rid) void goto(`/runs/${rid}`);
		}}
	>
		<span class="inline-block h-2 w-2 flex-shrink-0 rounded-full {t.dot}"></span>
		<span class={labelCls}>{label}</span>
		<span class="text-text-tertiary">{t.label}</span>
		{#if fanout}
			<span class="tabular-nums text-text-tertiary">· {fanout}</span>
		{/if}
		{#if rid}
			<ExternalLink size={12} class="text-text-tertiary" aria-hidden="true" />
		{/if}
	</button>
{/snippet}

<div class="border-t border-border px-4 py-3">
	<h3 class="mb-2 text-xs font-semibold tracking-wide text-text-tertiary uppercase">Flow nodes</h3>
	<ol class="flex flex-col gap-1.5">
		{#each status.nodes as node (node.id)}
			{@const rid = runIdOf(node)}
			{@const err = failureText(node)}
			<li class="flex flex-col gap-0.5">
				{@render row(node.id, `font-mono ${tone(node).text}`, tone(node), fanoutLabel(node), rid)}
				{#if err}
					<!-- Surface the failure reason inline so a node that never produced a
					     clickable run (e.g. its script was deleted) is still diagnosable. -->
					<p class="ml-4 break-words font-mono text-xs text-error">{err}</p>
				{/if}
			</li>
		{/each}
	</ol>
	{#if status.on_error}
		{@const rid = runIdOf(status.on_error)}
		{@const err = failureText(status.on_error)}
		<div class="mt-2 flex flex-col gap-0.5 border-t border-border-subtle pt-2">
			{@render row('error handler', 'text-text-tertiary', tone(status.on_error), null, rid)}
			{#if err}
				<p class="ml-4 break-words font-mono text-xs text-error">{err}</p>
			{/if}
		</div>
	{/if}
</div>
