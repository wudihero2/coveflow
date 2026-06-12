<script lang="ts">
	import { untrack } from 'svelte';
	import type { Backoff, RetryPolicy } from '$lib/types';

	interface Props {
		/** Current retry policy (undefined = no retry). */
		value: RetryPolicy | undefined;
		/** Called when the user commits a change. */
		onCommit: (value: RetryPolicy | undefined) => void;
	}
	let { value, onCommit }: Props = $props();

	// Seeded once; the parent re-keys ({#key node.id}) so switching nodes
	// re-seeds. Edits flow out only via onCommit (single source of truth = parent).
	let enabled = $state(untrack(() => value !== undefined));
	let maxAttempts = $state(untrack(() => value?.max_attempts ?? 2));
	let backoffKind = $state<Backoff['kind']>(untrack(() => value?.backoff.kind ?? 'fixed'));
	let delayMs = $state(
		untrack(() => (value?.backoff.kind === 'fixed' ? value.backoff.delay_ms : 1000))
	);
	let baseMs = $state(
		untrack(() => (value?.backoff.kind === 'exponential' ? value.backoff.base_ms : 1000))
	);
	let factor = $state(
		untrack(() => (value?.backoff.kind === 'exponential' ? value.backoff.factor : 2))
	);
	let jitter = $state(
		untrack(() =>
			value?.backoff.kind === 'exponential' && value.backoff.jitter != null
				? value.backoff.jitter
				: 0
		)
	);

	const FIELD = 'rounded-md border border-border bg-surface-raised px-2 py-1 text-xs text-text';

	// Coerce + clamp the form fields into a valid policy. max_attempts is the
	// number of retries after the first failure (engine: retries while
	// attempts < max_attempts), so it must be >= 1.
	function build(): RetryPolicy {
		const attempts = Math.max(1, Math.floor(maxAttempts) || 1);
		const backoff: Backoff =
			backoffKind === 'fixed'
				? { kind: 'fixed', delay_ms: Math.max(0, Math.floor(delayMs) || 0) }
				: {
						kind: 'exponential',
						base_ms: Math.max(0, Math.floor(baseMs) || 0),
						factor: Math.max(1, Math.floor(factor) || 1),
						...(jitter > 0 ? { jitter: Math.min(1, jitter) } : {})
					};
		return { max_attempts: attempts, backoff };
	}

	function emit(): void {
		onCommit(enabled ? build() : undefined);
	}
</script>

<div class="flex flex-col gap-1.5">
	<label class="flex items-center gap-2 text-xs text-text">
		<input type="checkbox" bind:checked={enabled} onchange={emit} />
		Retry on failure
	</label>

	{#if enabled}
		<div class="flex flex-col gap-2 rounded-md border border-border bg-surface-alt p-2">
			<label class="flex items-center justify-between gap-2">
				<span class="text-xs text-text-tertiary">max retries</span>
				<input
					class="{FIELD} w-20 text-right tabular-nums"
					type="number"
					min="1"
					step="1"
					bind:value={maxAttempts}
					onchange={emit}
				/>
			</label>

			<label class="flex items-center justify-between gap-2">
				<span class="text-xs text-text-tertiary">backoff</span>
				<select class="{FIELD} w-32" bind:value={backoffKind} onchange={emit}>
					<option value="fixed">fixed</option>
					<option value="exponential">exponential</option>
				</select>
			</label>

			{#if backoffKind === 'fixed'}
				<label class="flex items-center justify-between gap-2">
					<span class="text-xs text-text-tertiary">delay (ms)</span>
					<input
						class="{FIELD} w-24 text-right tabular-nums"
						type="number"
						min="0"
						step="100"
						bind:value={delayMs}
						onchange={emit}
					/>
				</label>
			{:else}
				<label class="flex items-center justify-between gap-2">
					<span class="text-xs text-text-tertiary">base (ms)</span>
					<input
						class="{FIELD} w-24 text-right tabular-nums"
						type="number"
						min="0"
						step="100"
						bind:value={baseMs}
						onchange={emit}
					/>
				</label>
				<label class="flex items-center justify-between gap-2">
					<span class="text-xs text-text-tertiary">factor</span>
					<input
						class="{FIELD} w-20 text-right tabular-nums"
						type="number"
						min="1"
						step="1"
						bind:value={factor}
						onchange={emit}
					/>
				</label>
				<label class="flex items-center justify-between gap-2">
					<span class="text-xs text-text-tertiary">jitter (0–1)</span>
					<input
						class="{FIELD} w-20 text-right tabular-nums"
						type="number"
						min="0"
						max="1"
						step="0.1"
						bind:value={jitter}
						onchange={emit}
					/>
				</label>
				<span class="text-[11px] text-text-tertiary">
					delay = base × factor<sup>attempt-1</sup>{jitter > 0 ? ', randomized by jitter' : ''}
				</span>
			{/if}
		</div>
	{/if}
</div>
