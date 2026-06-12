<script lang="ts">
	import { goto } from '$app/navigation';
	import MetaField from '$lib/components/common/MetaField.svelte';
	import { encodePath } from '$lib/services/url';
	import type { RunResponse } from '$lib/types';
	import { formatDuration } from '$lib/utils/format-duration';
	import { formatAbsolute } from '$lib/utils/format-time';
	import { displayTz } from '$lib/stores/timezone.svelte';

	interface Props {
		run: RunResponse;
	}

	let { run }: Props = $props();

	// Link the Script field to its editor: script runs → the script editor, flow
	// runs → the flow editor. null (preview / maintenance / no path) renders plain.
	const scriptHref = $derived.by(() => {
		const p = run.script_path;
		if (!p) return null;
		if (run.kind === 'script' || run.kind === 'preview') return `/scripts/edit/${encodePath(p)}`;
		if (run.kind === 'flow' || run.kind === 'flow_preview') return `/flows/edit/${encodePath(p)}`;
		return null;
	});

	// Absolute, to the second (runs can be seconds apart), in the global display
	// timezone (backend is always UTC). Reads displayTz.value so it re-renders when
	// the user switches the global tz.
	function formatDate(iso: string | null): string {
		return formatAbsolute(iso, displayTz.value);
	}

</script>

<dl class="space-y-5 px-5 py-5 text-xs">
	<MetaField label="Kind">
		<span class="text-sm text-text">{run.kind}</span>
	</MetaField>

	{#if run.script_path}
		<MetaField label="Script">
			{#if scriptHref}
				<button
					type="button"
					class="break-all text-left font-mono text-sm text-accent hover:underline"
					onclick={() => void goto(scriptHref)}
				>
					{run.script_path}
				</button>
			{:else}
				<span class="break-all font-mono text-sm text-text">{run.script_path}</span>
			{/if}
		</MetaField>
	{/if}

	{#if run.script_hash}
		<MetaField label="Hash">
			<span class="font-mono text-sm text-text-secondary" title={run.script_hash}>
				{run.script_hash.slice(0, 16)}…
			</span>
		</MetaField>
	{/if}

	{#if run.language}
		<MetaField label="Language">
			<span class="text-sm text-text">{run.language}</span>
		</MetaField>
	{/if}

	<MetaField label="Tag">
		<span class="text-sm text-text">{run.tag}</span>
	</MetaField>

	<MetaField label="Resources">
		<span class="font-mono text-sm text-text-secondary">
			{run.cpus} cpu · {run.memory_mb} MB mem · {run.disk_mb} MB disk
		</span>
	</MetaField>

	{#if run.requirements?.length}
		<MetaField label="Requirements">
			<div class="flex flex-wrap gap-1">
				{#each run.requirements as req (req)}
					<span class="rounded bg-surface-alt px-1.5 py-0.5 font-mono text-xs text-text-secondary"
						>{req}</span
					>
				{/each}
			</div>
		</MetaField>
	{/if}

	{#if run.timeout !== null}
		<MetaField label="Timeout">
			<span class="text-sm text-text">{run.timeout}s</span>
		</MetaField>
	{/if}

	<MetaField label="Created by">
		<span class="text-sm text-text">{run.created_by}</span>
	</MetaField>

	<MetaField label="Created">
		<time class="text-sm text-text" datetime={run.created_at} title={run.created_at}>
			{formatDate(run.created_at)}
		</time>
	</MetaField>

	{#if run.started_at}
		<MetaField label="Started">
			<time class="text-sm text-text" datetime={run.started_at} title={run.started_at}>
				{formatDate(run.started_at)}
			</time>
		</MetaField>
	{/if}

	{#if run.completed_at}
		<MetaField label="Completed">
			<time class="text-sm text-text" datetime={run.completed_at} title={run.completed_at}>
				{formatDate(run.completed_at)}
			</time>
		</MetaField>
	{/if}

	{#if run.duration_ms !== null}
		<MetaField label="Duration">
			<span class="font-mono text-sm text-text">{formatDuration(run.duration_ms)}</span>
		</MetaField>
	{/if}

	{#if run.parent_run}
		<MetaField label="Parent run">
			<button type="button" class="font-mono text-sm text-accent hover:underline" onclick={() => void goto(`/runs/${run.parent_run}`)}>
				{run.parent_run.slice(0, 8)}
			</button>
		</MetaField>
	{/if}

	{#if run.root_run && run.root_run !== run.id}
		<MetaField label="Root run">
			<button type="button" class="font-mono text-sm text-accent hover:underline" onclick={() => void goto(`/runs/${run.root_run}`)}>
				{run.root_run.slice(0, 8)}
			</button>
		</MetaField>
	{/if}
</dl>
