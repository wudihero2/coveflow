<script lang="ts">
	import { Clock, ChevronDown } from '@lucide/svelte';

	interface TimeRange {
		after?: number;
		before?: number;
	}

	interface Props {
		value: TimeRange;
		class?: string;
	}

	let { value = $bindable({}), class: className = '' }: Props = $props();

	type Mode = 'relative' | 'absolute';
	let mode = $state<Mode>('relative');
	let open = $state(false);

	const relativeOptions = [
		{ label: 'Last 5m', ms: 5 * 60 * 1000 },
		{ label: 'Last 15m', ms: 15 * 60 * 1000 },
		{ label: 'Last 1h', ms: 60 * 60 * 1000 },
		{ label: 'Last 6h', ms: 6 * 60 * 60 * 1000 },
		{ label: 'Last 24h', ms: 24 * 60 * 60 * 1000 },
		{ label: 'Last 7d', ms: 7 * 24 * 60 * 60 * 1000 },
		{ label: 'All time', ms: 0 }
	];

	// Initialize the trigger's selection/label from the incoming `value` so it
	// reflects what the parent actually requested (e.g. "Last 15m") instead of a
	// hardcoded default — otherwise the picker would read "Last 24h" while a much
	// shorter window is in effect, making it look like entry fetches 24h of logs.
	function initFromValue(v: TimeRange): { ms: number; label: string } {
		if (v.after == null && v.before == null) {
			return { ms: 0, label: 'All time' };
		}
		// Relative (open-ended) range: map to the closest preset for the label.
		if (v.after != null && v.before == null) {
			const elapsed = Date.now() - v.after;
			const presets = relativeOptions.filter((o) => o.ms > 0);
			const closest = presets.reduce((a, b) =>
				Math.abs(b.ms - elapsed) < Math.abs(a.ms - elapsed) ? b : a
			);
			return { ms: closest.ms, label: closest.label };
		}
		// Bounded range → treat as a custom/absolute selection.
		return { ms: -1, label: 'Custom' };
	}

	const initial = initFromValue(value);
	let selectedRelativeMs = $state(initial.ms);
	let absoluteFrom = $state('');
	let absoluteTo = $state('');

	let displayLabel = $state(initial.label);

	function selectRelative(opt: { label: string; ms: number }): void {
		selectedRelativeMs = opt.ms;
		displayLabel = opt.label;
		value = opt.ms > 0 ? { after: Date.now() - opt.ms, before: undefined } : {};
		open = false;
	}

	function applyAbsolute(): void {
		const after = absoluteFrom ? new Date(absoluteFrom).getTime() : undefined;
		const before = absoluteTo ? new Date(absoluteTo).getTime() : undefined;
		value = { after, before };

		const parts: string[] = [];
		if (absoluteFrom) parts.push(formatLocalDateTime(absoluteFrom));
		if (absoluteTo) parts.push(formatLocalDateTime(absoluteTo));
		displayLabel = parts.length > 0 ? parts.join(' → ') : 'Custom';
		open = false;
	}

	function formatLocalDateTime(dt: string): string {
		const d = new Date(dt);
		if (isNaN(d.getTime())) return dt;
		const pad = (n: number) => n.toString().padStart(2, '0');
		return `${pad(d.getMonth() + 1)}/${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
	}

	function handleClickOutside(e: MouseEvent): void {
		const target = e.target as HTMLElement;
		if (!target.closest('.time-range-picker')) {
			open = false;
		}
	}
</script>

<svelte:window onclick={handleClickOutside} />

<div class="time-range-picker relative {className}">
	<!-- Trigger button — same height/style as Select -->
	<button
		type="button"
		onclick={() => (open = !open)}
		class="flex h-10 w-full items-center gap-2 rounded-md border border-border bg-surface px-3 text-sm text-text outline-none transition hover:border-border-strong focus:border-accent focus:ring-1 focus:ring-accent"
	>
		<Clock size={14} class="shrink-0 text-text-tertiary" />
		<span class="min-w-0 flex-1 truncate text-left">{displayLabel}</span>
		<ChevronDown size={14} class="shrink-0 text-text-tertiary transition {open ? 'rotate-180' : ''}" />
	</button>

	<!-- Popover -->
	{#if open}
		<div
			class="absolute left-0 top-full z-20 mt-1 w-72 rounded-lg border border-border bg-surface-raised p-3 shadow-lg"
		>
			<!-- Mode tabs -->
			<div class="mb-3 flex gap-0.5 rounded-md bg-surface-alt p-0.5" role="tablist">
				<button
					type="button"
					role="tab"
					aria-selected={mode === 'relative'}
					class="flex-1 rounded px-3 py-1.5 text-xs font-medium transition
						{mode === 'relative'
						? 'bg-surface-raised text-text shadow-sm'
						: 'text-text-secondary hover:text-text'}"
					onclick={() => (mode = 'relative')}
				>
					Relative
				</button>
				<button
					type="button"
					role="tab"
					aria-selected={mode === 'absolute'}
					class="flex-1 rounded px-3 py-1.5 text-xs font-medium transition
						{mode === 'absolute'
						? 'bg-surface-raised text-text shadow-sm'
						: 'text-text-secondary hover:text-text'}"
					onclick={() => (mode = 'absolute')}
				>
					Absolute
				</button>
			</div>

			{#if mode === 'relative'}
				<!-- Relative options list -->
				<div class="flex flex-col">
					{#each relativeOptions as opt (opt.ms)}
						<button
							type="button"
							class="rounded px-3 py-1.5 text-left text-sm transition
								{opt.ms === selectedRelativeMs
								? 'bg-accent-subtle font-medium text-accent'
								: 'text-text hover:bg-surface-alt'}"
							onclick={() => selectRelative(opt)}
						>
							{opt.label}
						</button>
					{/each}
				</div>
			{:else}
				<!-- Absolute: Apply button first so the native calendar can't block it -->
				<div class="flex flex-col gap-3">
					<button
						type="button"
						onclick={applyAbsolute}
						disabled={!absoluteFrom && !absoluteTo}
						class="h-8 rounded-md bg-accent px-4 text-xs font-medium text-white transition hover:bg-accent-hover disabled:cursor-not-allowed disabled:opacity-50"
					>
						Apply
					</button>
					<label class="flex flex-col gap-1">
						<span class="text-xs font-medium text-text-secondary">From</span>
						<input
							type="datetime-local"
							step="1"
							bind:value={absoluteFrom}
							class="h-9 w-full rounded-md border border-border bg-surface px-3 text-sm text-text outline-none focus:border-accent focus:ring-1 focus:ring-accent"
						/>
					</label>
					<label class="flex flex-col gap-1">
						<span class="text-xs font-medium text-text-secondary">To</span>
						<input
							type="datetime-local"
							step="1"
							bind:value={absoluteTo}
							class="h-9 w-full rounded-md border border-border bg-surface px-3 text-sm text-text outline-none focus:border-accent focus:ring-1 focus:ring-accent"
						/>
					</label>
				</div>
			{/if}
		</div>
	{/if}
</div>
