<script lang="ts">
	import { untrack } from 'svelte';
	import { X } from '@lucide/svelte';
	import IconButton from '$lib/components/common/IconButton.svelte';
	import type { TeamListItem } from '$lib/types';
	import { runOptions } from './run-options/store.svelte';
	import { SOFT_LIMITS } from './run-options/types';

	interface Props {
		open: boolean;
		teams: TeamListItem[];
		teamsLoading: boolean;
		teamsError: boolean;
		anchor?: HTMLElement | null;
		onClose: () => void;
	}

	let { open, teams, teamsLoading, teamsError, anchor = null, onClose }: Props = $props();

	let panel = $state<HTMLDivElement | undefined>();
	let firstInput = $state<HTMLTextAreaElement | undefined>();
	let argsText = $state(serializeArgs(runOptions.args));
	let argsError = $state<string | null>(null);

	function serializeArgs(value: unknown): string {
		if (value === undefined) return '';
		try {
			return JSON.stringify(value, null, 2);
		} catch {
			return '';
		}
	}

	function applyArgs(value: string): void {
		argsText = value;
		const trimmed = value.trim();
		if (trimmed === '') {
			runOptions.args = undefined;
			argsError = null;
			return;
		}
		try {
			runOptions.args = JSON.parse(trimmed);
			argsError = null;
		} catch (e) {
			argsError = e instanceof Error ? e.message : 'Invalid JSON';
			runOptions.args = undefined;
		}
	}

	function applyNumber(
		field: 'timeout' | 'priority' | 'cpus' | 'memoryMb' | 'diskMb',
		raw: string
	): void {
		const trimmed = raw.trim();
		if (trimmed === '') {
			runOptions[field] = undefined;
			return;
		}
		const num = Number(trimmed);
		if (Number.isNaN(num)) {
			runOptions[field] = undefined;
			return;
		}
		runOptions[field] = num;
	}

	function applyTag(value: string): void {
		const trimmed = value.trim();
		runOptions.tag = trimmed === '' ? undefined : trimmed;
	}

	function applyTeam(value: string): void {
		runOptions.teamOwner = value === '' ? null : value;
	}

	const tagError = $derived.by(() => {
		if (!runOptions.tag) return null;
		if (runOptions.tag.length >= 64) return 'Length must be < 64';
		return null;
	});

	const timeoutError = $derived.by(() => {
		if (runOptions.timeout === undefined) return null;
		const { min, max } = SOFT_LIMITS.timeout;
		if (runOptions.timeout < min || runOptions.timeout > max) return `Range: ${min} – ${max}`;
		return null;
	});

	const priorityError = $derived.by(() => {
		if (runOptions.priority === undefined) return null;
		const { min, max } = SOFT_LIMITS.priority;
		if (runOptions.priority < min || runOptions.priority > max) return `Range: ${min} – ${max}`;
		return null;
	});

	const cpusError = $derived.by(() => {
		if (runOptions.cpus === undefined) return null;
		const { min, max } = SOFT_LIMITS.cpus;
		if (runOptions.cpus < min || runOptions.cpus > max) return `Range: ${min} – ${max}`;
		return null;
	});

	const memoryError = $derived.by(() => {
		if (runOptions.memoryMb === undefined) return null;
		const { min, max } = SOFT_LIMITS.memoryMb;
		if (runOptions.memoryMb < min || runOptions.memoryMb > max) return `Range: ${min} – ${max}`;
		return null;
	});

	const diskError = $derived.by(() => {
		if (runOptions.diskMb === undefined) return null;
		const { min, max } = SOFT_LIMITS.diskMb;
		if (runOptions.diskMb < min || runOptions.diskMb > max) return `Range: ${min} – ${max}`;
		return null;
	});

	function inputClass(error: string | null): string {
		return [
			'mt-1 w-full rounded-md border bg-surface px-2.5 py-1.5 text-sm text-text outline-none transition focus:ring-1',
			error
				? 'border-error focus:border-error focus:ring-error/40'
				: 'border-border focus:border-accent focus:ring-accent'
		].join(' ');
	}

	function focusableElements(root: HTMLElement): HTMLElement[] {
		return Array.from(
			root.querySelectorAll<HTMLElement>(
				'button:not([disabled]), input:not([disabled]), textarea:not([disabled]), select:not([disabled]), [tabindex]:not([tabindex="-1"])'
			)
		);
	}

	// Re-sync argsText from the store ONLY when the popover opens (E11), reading
	// args untracked. Otherwise this effect would also fire whenever applyArgs
	// writes runOptions.args (e.g. setting it to undefined on invalid JSON),
	// clobbering the textarea mid-edit.
	let wasOpen = false;
	$effect(() => {
		if (open && !wasOpen) {
			argsText = serializeArgs(untrack(() => runOptions.args));
			argsError = null;
			queueMicrotask(() => firstInput?.focus());
		}
		wasOpen = open;
	});

	// Document-level listeners while the popover is open (close on outside click,
	// Escape, and Tab focus-trap).
	$effect(() => {
		if (!open) return;

		function onDocClick(e: MouseEvent) {
			if (!panel) return;
			const target = e.target as Node | null;
			if (target && (panel.contains(target) || anchor?.contains(target))) return;
			onClose();
		}
		function onKey(e: KeyboardEvent) {
			if (e.key === 'Escape') {
				onClose();
				return;
			}
			if (e.key !== 'Tab' || !panel) return;
			// E1: focus trap — Tab stays inside the popover.
			const f = focusableElements(panel);
			if (f.length === 0) return;
			const first = f[0];
			const last = f[f.length - 1];
			const active = document.activeElement as HTMLElement | null;
			if (e.shiftKey && active === first) {
				e.preventDefault();
				last.focus();
			} else if (!e.shiftKey && active === last) {
				e.preventDefault();
				first.focus();
			}
		}
		document.addEventListener('mousedown', onDocClick);
		document.addEventListener('keydown', onKey);
		return () => {
			document.removeEventListener('mousedown', onDocClick);
			document.removeEventListener('keydown', onKey);
		};
	});

	const teamOptions = $derived.by(() => {
		const opts = [{ label: '— (no team)', value: '' }];
		for (const team of teams) {
			opts.push({ label: team.name, value: team.name });
		}
		return opts;
	});
</script>

{#if open}
	<div
		bind:this={panel}
		role="dialog"
		aria-label="Run options"
		class="absolute right-0 top-full z-30 mt-1 w-[min(420px,calc(100vw-1rem))] rounded-lg border border-border bg-surface-raised shadow-lg"
	>
		<div class="flex items-center justify-between border-b border-border px-4 py-2.5">
			<h3 class="text-sm font-semibold text-text">Run options</h3>
			<IconButton aria-label="Close" onclick={onClose}>
				<X size={14} />
			</IconButton>
		</div>

		<div class="border-b border-border bg-warning/10 px-4 py-2 text-xs text-text-secondary">
			⚠ These settings affect system resources and team quota. Use with care.
		</div>

		<div class="space-y-3 px-4 py-3">
			<!-- Args (JSON) -->
			<div>
				<label class="text-xs font-medium text-text-secondary" for="run-opt-args">Args (JSON)</label>
				<textarea
					id="run-opt-args"
					bind:this={firstInput}
					rows={4}
					placeholder={'{ }'}
					value={argsText}
					oninput={(e) => applyArgs(e.currentTarget.value)}
					class="mt-1 w-full rounded-md border bg-surface px-2.5 py-1.5 font-mono text-xs text-text outline-none transition focus:ring-1 {argsError
						? 'border-error focus:border-error focus:ring-error/40'
						: 'border-border focus:border-accent focus:ring-accent'}"
				></textarea>
				<p class="mt-1 text-[11px] {argsError ? 'text-error' : 'text-text-tertiary'}">
					{argsError ?? 'Must be valid JSON object'}
				</p>
			</div>

			<!-- Tag + Timeout -->
			<div class="grid grid-cols-2 gap-3">
				<div>
					<label class="text-xs font-medium text-text-secondary" for="run-opt-tag">Tag</label>
					<input
						id="run-opt-tag"
						type="text"
						placeholder="default"
						value={runOptions.tag ?? ''}
						oninput={(e) => applyTag(e.currentTarget.value)}
						class={inputClass(tagError)}
					/>
					<p class="mt-1 text-[11px] {tagError ? 'text-error' : 'text-text-tertiary'}">
						{tagError ?? 'Worker scheduling label'}
					</p>
				</div>
				<div>
					<label class="text-xs font-medium text-text-secondary" for="run-opt-timeout"
						>Timeout (s)</label
					>
					<input
						id="run-opt-timeout"
						type="number"
						min={SOFT_LIMITS.timeout.min}
						max={SOFT_LIMITS.timeout.max}
						placeholder={String(SOFT_LIMITS.timeout.default)}
						value={runOptions.timeout ?? ''}
						oninput={(e) => applyNumber('timeout', e.currentTarget.value)}
						class={inputClass(timeoutError)}
					/>
					<p class="mt-1 text-[11px] {timeoutError ? 'text-error' : 'text-text-tertiary'}">
						{timeoutError ??
							`Seconds (${SOFT_LIMITS.timeout.min} – ${SOFT_LIMITS.timeout.max}, default ${SOFT_LIMITS.timeout.default})`}
					</p>
				</div>
			</div>

			<!-- Priority + CPUs + Memory -->
			<div class="grid grid-cols-3 gap-3">
				<div>
					<label class="text-xs font-medium text-text-secondary" for="run-opt-priority"
						>Priority</label
					>
					<input
						id="run-opt-priority"
						type="number"
						min={SOFT_LIMITS.priority.min}
						max={SOFT_LIMITS.priority.max}
						placeholder={String(SOFT_LIMITS.priority.default)}
						value={runOptions.priority ?? ''}
						oninput={(e) => applyNumber('priority', e.currentTarget.value)}
						class={inputClass(priorityError)}
					/>
					<p class="mt-1 text-[11px] {priorityError ? 'text-error' : 'text-text-tertiary'}">
						{priorityError ?? `${SOFT_LIMITS.priority.min} – ${SOFT_LIMITS.priority.max}`}
					</p>
				</div>
				<div>
					<label class="text-xs font-medium text-text-secondary" for="run-opt-cpus">CPUs</label>
					<input
						id="run-opt-cpus"
						type="number"
						step="0.1"
						min={SOFT_LIMITS.cpus.min}
						max={SOFT_LIMITS.cpus.max}
						placeholder={String(SOFT_LIMITS.cpus.default)}
						value={runOptions.cpus ?? ''}
						oninput={(e) => applyNumber('cpus', e.currentTarget.value)}
						class={inputClass(cpusError)}
					/>
					<p class="mt-1 text-[11px] {cpusError ? 'text-error' : 'text-text-tertiary'}">
						{cpusError ?? `Float, ${SOFT_LIMITS.cpus.min} – ${SOFT_LIMITS.cpus.max}`}
					</p>
				</div>
				<div>
					<label class="text-xs font-medium text-text-secondary" for="run-opt-mem">Memory (MB)</label>
					<input
						id="run-opt-mem"
						type="number"
						min={SOFT_LIMITS.memoryMb.min}
						max={SOFT_LIMITS.memoryMb.max}
						placeholder={String(SOFT_LIMITS.memoryMb.default)}
						value={runOptions.memoryMb ?? ''}
						oninput={(e) => applyNumber('memoryMb', e.currentTarget.value)}
						class={inputClass(memoryError)}
					/>
					<p class="mt-1 text-[11px] {memoryError ? 'text-error' : 'text-text-tertiary'}">
						{memoryError ?? `MB, ${SOFT_LIMITS.memoryMb.min} – ${SOFT_LIMITS.memoryMb.max}`}
					</p>
				</div>
			</div>

			<!-- Disk + Team owner -->
			<div class="grid grid-cols-2 gap-3">
				<div>
					<label class="text-xs font-medium text-text-secondary" for="run-opt-disk">Disk (MB)</label>
					<input
						id="run-opt-disk"
						type="number"
						min={SOFT_LIMITS.diskMb.min}
						max={SOFT_LIMITS.diskMb.max}
						placeholder={String(SOFT_LIMITS.diskMb.default)}
						value={runOptions.diskMb ?? ''}
						oninput={(e) => applyNumber('diskMb', e.currentTarget.value)}
						class={inputClass(diskError)}
					/>
					<p class="mt-1 text-[11px] {diskError ? 'text-error' : 'text-text-tertiary'}">
						{diskError ?? `MB, ${SOFT_LIMITS.diskMb.min} – ${SOFT_LIMITS.diskMb.max}`}
					</p>
				</div>
				<div>
					<label class="text-xs font-medium text-text-secondary" for="run-opt-team">Team owner</label>
					<select
						id="run-opt-team"
						disabled={teamsLoading || teamsError}
						value={runOptions.teamOwner ?? ''}
						onchange={(e) => applyTeam(e.currentTarget.value)}
						class={inputClass(null)}
					>
						{#each teamOptions as opt (opt.value)}
							<option value={opt.value}>{opt.label}</option>
						{/each}
					</select>
					<p class="mt-1 text-[11px] {teamsError ? 'text-error' : 'text-text-tertiary'}">
						{teamsError ? 'Failed to load teams' : teamsLoading ? 'Loading teams…' : ' '}
					</p>
				</div>
			</div>
		</div>
	</div>
{/if}
