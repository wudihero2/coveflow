<script lang="ts">
	import { CalendarClock, ChevronRight, Hand } from '@lucide/svelte';

	import Badge from '$lib/components/common/Badge.svelte';
	import MetaField from '$lib/components/common/MetaField.svelte';
	import { displayTz } from '$lib/stores/timezone.svelte';
	import type { RunContext } from '$lib/types';
	import { formatAbsolute } from '$lib/utils/format-time';

	interface Props {
		context: RunContext;
	}

	let { context }: Props = $props();

	let open = $state(false);

	// UTC timestamps render in the user's display timezone (title keeps the raw
	// ISO). `ds`/`ts` are already computed in the schedule timezone, so show them
	// verbatim — that's the whole point of those fields.
	function tz(iso: string): string {
		return formatAbsolute(iso, displayTz.value);
	}

	const stepIds = $derived(Object.keys(context.steps ?? {}));
	const hasFlowInput =
		$derived(context.flow_input !== null && context.flow_input !== undefined);
</script>

<section class="border-t border-border px-5 py-4 text-xs">
	<button
		type="button"
		class="flex w-full items-center gap-2 text-left"
		aria-expanded={open}
		onclick={() => (open = !open)}
	>
		<ChevronRight
			size={14}
			class="shrink-0 text-text-tertiary transition-transform {open ? 'rotate-90' : ''}"
		/>
		<span class="font-medium uppercase tracking-wider text-text-tertiary">Run context</span>
		{#if context.is_scheduled}
			<Badge variant="info" class="gap-1">
				<CalendarClock size={11} />
				{context.schedule_name ?? 'scheduled'}
			</Badge>
		{:else}
			<Badge variant="ghost" class="gap-1">
				<Hand size={11} />
				manual
			</Badge>
		{/if}
	</button>

	{#if open}
		<dl class="mt-4 space-y-5">
			<MetaField label="Logical date">
				<time class="font-mono text-sm text-text" datetime={context.logical_date} title={context.logical_date}>
					{tz(context.logical_date)}
				</time>
			</MetaField>

			{#if context.is_scheduled}
				<MetaField label="Data interval">
					<div class="flex flex-col font-mono text-sm text-text-secondary">
						<span title={context.data_interval_start}>{tz(context.data_interval_start)}</span>
						<span class="text-text-tertiary">↳ {tz(context.data_interval_end)}</span>
					</div>
				</MetaField>
			{/if}

			<MetaField label="ds / ts">
				<div class="flex flex-col font-mono text-sm text-text-secondary">
					<span>{context.ds}</span>
					<span class="break-all" title="In schedule timezone {context.timezone}">{context.ts}</span>
				</div>
			</MetaField>

			<MetaField label="Triggered at">
				<time class="text-sm text-text-secondary" datetime={context.triggered_at} title={context.triggered_at}>
					{tz(context.triggered_at)}
				</time>
			</MetaField>

			{#if context.flow_path}
				<MetaField label="Flow">
					<span class="break-all font-mono text-sm text-text">{context.flow_path}</span>
				</MetaField>
			{/if}

			{#if hasFlowInput}
				<MetaField label="Flow input (params)">
					<pre class="overflow-x-auto rounded bg-surface-alt px-2 py-1.5 font-mono text-xs text-text-secondary">{JSON.stringify(context.flow_input, null, 2)}</pre>
				</MetaField>
			{/if}

			{#if stepIds.length}
				<MetaField label="Upstream steps">
					<div class="flex flex-wrap gap-1">
						{#each stepIds as id (id)}
							<span class="rounded bg-surface-alt px-1.5 py-0.5 font-mono text-xs text-text-secondary">{id}</span>
						{/each}
					</div>
				</MetaField>
			{/if}
		</dl>
	{/if}
</section>
