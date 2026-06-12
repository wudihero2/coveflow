<script lang="ts">
	import {
		TriangleAlert,
		CircleCheck,
		CircleDot,
		Copy,
		Download,
		LoaderCircle,
		MessageSquareDashed,
		CircleX,
		ChevronRight
	} from '@lucide/svelte';
	import { SvelteSet } from 'svelte/reactivity';
	import IconButton from '$lib/components/common/IconButton.svelte';
	import type { LogLevelName } from '$lib/services/api';
	import { pollRunLogs } from '$lib/services/polling';
	import { toastError, toastInfo, toastSuccess } from '$lib/toast';
	import type { LogEntry, RunResultEvent } from '$lib/types';
	import { displayTz } from '$lib/stores/timezone.svelte';
	import { formatAbsolute } from '$lib/utils/format-time';
	import { appendManyCapped } from './log/buffer';
	import {
		entriesToPlainText,
		formatEntryFields,
		levelBorderClass,
		levelClass,
		levelLabel
	} from './log/format';

	/**
	 * Local enrichment of LogEntry with a stable monotonic id. Needed because
	 * `lines` is capped at 10k and trims from the front, which would shift any
	 * positional index used to track collapsed state.
	 */
	interface IdentifiedLogEntry extends LogEntry {
		_id: number;
	}

	type Status = 'idle' | 'connecting' | 'streaming' | 'success' | 'failure' | 'error';

	interface Props {
		workspaceId: string;
		runId: string | null;
		minLevel?: number;
		onResult?: (event: RunResultEvent) => void;
		onError?: (message: string) => void;
		class?: string;
	}

	let {
		workspaceId,
		runId,
		minLevel = 3,
		onResult,
		onError,
		class: className = ''
	}: Props = $props();

	// $state.raw avoids per-element Proxy overhead for 10k entries — we reassign
	// the array reference instead of mutating in place. `readonly` makes the
	// no-mutation contract explicit and lines up with appendManyCapped's signature.
	let lines = $state.raw<readonly IdentifiedLogEntry[]>([]);
	let status = $state<Status>('idle');
	let autoScrollPaused = $state(false);
	let pendingCount = $state(0);
	let container = $state<HTMLDivElement | undefined>();
	/** IDs of entries whose fields block is currently expanded. Default is
	 * collapsed — users opt in to seeing structured fields per entry.
	 * SvelteSet is reactive on its own, no $state wrapper needed. */
	const expandedIds = new SvelteSet<number>();
	let nextEntryId = 0;

	function toggleExpanded(id: number): void {
		if (expandedIds.has(id)) {
			expandedIds.delete(id);
		} else {
			expandedIds.add(id);
		}
	}

	const LEVEL_NAMES: Record<number, LogLevelName> = {
		1: 'TRACE',
		2: 'DEBUG',
		3: 'INFO',
		4: 'WARN',
		5: 'ERROR'
	};

	function levelToName(level: number): LogLevelName {
		return LEVEL_NAMES[level] ?? 'INFO';
	}

	function sanitizeFilename(s: string): string {
		return s.replace(/[^a-zA-Z0-9._-]/g, '_');
	}

	function isAtBottom(): boolean {
		if (!container) return true;
		return container.scrollHeight - container.scrollTop - container.clientHeight < 40;
	}

	function scrollToBottom(): void {
		if (!container) return;
		requestAnimationFrame(() => {
			if (!container) return;
			container.scrollTop = container.scrollHeight;
		});
	}

	function handleScroll(): void {
		if (!container) return;
		if (isAtBottom()) {
			autoScrollPaused = false;
			pendingCount = 0;
		} else {
			autoScrollPaused = true;
		}
	}

	function jumpToBottom(): void {
		autoScrollPaused = false;
		pendingCount = 0;
		scrollToBottom();
	}

	// Polling lifecycle: re-poll whenever runId/workspaceId/minLevel change.
	$effect(() => {
		// Reset state on every restart.
		lines = [];
		pendingCount = 0;
		autoScrollPaused = false;
		expandedIds.clear();

		if (!runId) {
			status = 'idle';
			return;
		}

		status = 'connecting';
		let toastedError = false; // one-shot guard so retries don't spam toasts

		const cleanup = pollRunLogs(workspaceId, runId, {
			level: levelToName(minLevel),
			onOpen() {
				const recoveredFromError = status === 'error';
				if (status === 'connecting' || status === 'error') status = 'streaming';
				if (recoveredFromError && toastedError) {
					toastInfo('Reconnected to log stream');
				}
				toastedError = false;
			},
			onLog(chunk) {
				if (status === 'connecting' || status === 'error') status = 'streaming';
				const identified: IdentifiedLogEntry[] = chunk.entries.map((e) => ({
					...e,
					_id: nextEntryId++
				}));
				lines = appendManyCapped(lines, identified);
				if (autoScrollPaused) {
					pendingCount += chunk.entries.length;
				} else {
					scrollToBottom();
				}
			},
			onResult(event) {
				status = event.success ? 'success' : 'failure';
				onResult?.(event);
			},
			onError(message) {
				status = 'error';
				if (!toastedError) {
					toastedError = true;
					onError?.(message);
				}
			}
		});

		return () => cleanup();
	});

	type StatusMeta = {
		icon: typeof LoaderCircle;
		label: string;
		color: string;
		spin: boolean;
		pulse: boolean;
	};

	const statusMeta = $derived.by<StatusMeta>(() => {
		switch (status) {
			case 'connecting':
				return {
					icon: LoaderCircle,
					label: 'Connecting…',
					color: 'text-text-tertiary',
					spin: true,
					pulse: false
				};
			case 'streaming':
				return {
					icon: CircleDot,
					label: `Running · ${lines.length} lines`,
					color: 'text-info',
					spin: false,
					pulse: true
				};
			case 'success':
				return {
					icon: CircleCheck,
					label: `Success · ${lines.length} lines`,
					color: 'text-success',
					spin: false,
					pulse: false
				};
			case 'failure':
				return {
					icon: CircleX,
					label: `Failed · ${lines.length} lines`,
					color: 'text-error',
					spin: false,
					pulse: false
				};
			case 'error':
				return {
					icon: TriangleAlert,
					label: `Connection error · ${lines.length} lines`,
					color: 'text-error',
					spin: false,
					pulse: false
				};
			default:
				return {
					icon: MessageSquareDashed,
					label: 'Idle',
					color: 'text-text-tertiary',
					spin: false,
					pulse: false
				};
		}
	});

	async function copyAll(): Promise<void> {
		try {
			const text = entriesToPlainText(lines);
			await navigator.clipboard.writeText(text);
			toastSuccess(`Copied ${lines.length} lines`);
		} catch (e) {
			toastError(e instanceof Error ? e.message : 'Copy failed');
		}
	}

	function downloadTxt(): void {
		const text = entriesToPlainText(lines);
		const blob = new Blob([text], { type: 'text/plain' });
		const url = URL.createObjectURL(blob);
		const a = document.createElement('a');
		a.href = url;
		a.download = `coveflow-run-${sanitizeFilename(runId ?? 'unknown')}.txt`;
		document.body.appendChild(a);
		a.click();
		document.body.removeChild(a);
		URL.revokeObjectURL(url);
	}

	const showStatusBar = $derived(status !== 'idle');
	const showEmptyNoRun = $derived(!runId);
	const showEmptyWaiting = $derived(
		!!runId && (status === 'connecting' || status === 'streaming') && lines.length === 0
	);
	const showEmptyNoOutput = $derived(
		!!runId && (status === 'success' || status === 'failure') && lines.length === 0
	);

	const liveDescription = $derived(`Log stream ${status}, ${lines.length} lines`);
</script>

<div class="flex min-h-0 flex-col {className}">
	{#if showStatusBar}
		<!-- Status bar — hidden when idle (no run yet) since the empty placeholder already speaks. -->
		<div
			role="status"
			class="flex items-center gap-2 border-b border-border bg-surface-alt px-3 py-1.5 text-xs"
		>
			<statusMeta.icon
				size={14}
				class="{statusMeta.color} {statusMeta.spin ? 'animate-spin' : ''} {statusMeta.pulse
					? 'animate-pulse'
					: ''}"
			/>
			<span class="font-medium {statusMeta.color}">{statusMeta.label}</span>
			<span class="sr-only">{liveDescription}</span>

			{#if autoScrollPaused}
				<span
					class="ml-2 rounded-md bg-surface-sunken px-2 py-0.5 text-[11px] text-text-tertiary"
					>Auto-scroll paused</span
				>
			{/if}

			<div class="flex-1"></div>

			<IconButton
				aria-label="Copy all logs"
				title="Copy all logs"
				onclick={copyAll}
				disabled={lines.length === 0}
			>
				<Copy size={14} />
			</IconButton>
			<IconButton
				aria-label="Download log as .txt"
				title="Download .txt"
				onclick={downloadTxt}
				disabled={lines.length === 0 || !runId}
			>
				<Download size={14} />
			</IconButton>
		</div>
	{/if}

	<!-- Log surface -->
	<div class="relative min-h-0 flex-1">
		<div
			bind:this={container}
			role="log"
			aria-live="polite"
			aria-relevant="additions"
			onscroll={handleScroll}
			class="absolute inset-0 overflow-y-auto bg-surface-sunken px-3 py-2 font-mono text-[13px] leading-snug"
		>
			{#if showEmptyNoRun}
				<div class="flex h-full flex-col items-center justify-center gap-2 text-text-tertiary">
					<MessageSquareDashed size={28} />
					<p class="text-sm">No run yet — click ▶ Run to start.</p>
				</div>
			{:else if showEmptyWaiting}
				<div class="flex h-full flex-col items-center justify-center gap-2 text-text-tertiary">
					<LoaderCircle size={20} class="animate-spin" />
					<p class="text-sm">Waiting for logs…</p>
				</div>
			{:else if showEmptyNoOutput}
				<div class="flex h-full items-center justify-center text-sm text-text-tertiary">
					(No log output)
				</div>
			{:else}
				{#snippet entryHeaderContent(entry: IdentifiedLogEntry)}
					<!--
						Only the level label carries the level colour. Timestamp/target
						stay tertiary, message stays default text — chasing colour-coded
						severity across the whole row was overwhelming for the common case.
					-->
					<span class="text-text-tertiary">{formatAbsolute(entry.ts, displayTz.value)}</span>
					<span class="ml-2 {levelClass(entry.level)}">{levelLabel(entry.level)}</span>
					{#if entry.target}
						<span class="ml-2 text-text-tertiary">[{entry.target}]</span>
					{/if}
					{#if entry.node_id}
						<span class="ml-2 text-text-tertiary">[{entry.node_id}]</span>
					{/if}
					<span class="ml-2 text-text">{entry.msg}</span>
				{/snippet}
				{#each lines as entry (entry._id)}
					{@const fields = formatEntryFields(entry.fields)}
					{@const hasFields = fields.length > 0}
					{@const isExpanded = expandedIds.has(entry._id)}
					{@const fieldsId = `entry-${entry._id}-fields`}
					<!--
						Each log row: subtle left accent bar (only WARN/ERROR are coloured),
						chevron toggle button on the left, then a non-button header so the
						text inside is freely selectable (triple-click, drag-select, copy).
					-->
					<div class="my-0.5 flex items-start gap-1 border-l-2 {levelBorderClass(
							entry.level
						)} pl-2">
						{#if hasFields}
							<button
								type="button"
								class="mt-0.5 shrink-0 rounded text-text-tertiary hover:bg-surface-alt/30 hover:text-text"
								aria-expanded={isExpanded}
								aria-controls={isExpanded ? fieldsId : undefined}
								aria-label={isExpanded ? 'Collapse fields' : 'Expand fields'}
								onclick={() => toggleExpanded(entry._id)}
							>
								<ChevronRight
									size={12}
									class="transition-transform duration-150 {isExpanded ? 'rotate-90' : ''}"
								/>
							</button>
						{:else}
							<!-- Spacer keeps message left-aligned with rows that have a chevron. -->
							<span class="mt-0.5 inline-block w-3 shrink-0"></span>
						{/if}
						<div class="min-w-0 flex-1">
							<div class="whitespace-pre-wrap break-words">
								{@render entryHeaderContent(entry)}
							</div>
							{#if hasFields && isExpanded}
								<div
									id={fieldsId}
									class="ml-3 mt-0.5 border-l border-border pl-3 text-text-secondary"
								>
									{#each fields as f (f.key)}
										{#if f.key === 'stdout'}
											<!--
												stdout: the user's print() output — calm, success-coloured.
												stderr: the user's script complained — warning-coloured.
												Both get a tiny uppercase label so the source is unambiguous.
											-->
											<div class="my-1">
												<div class="text-[10px] uppercase tracking-wider text-text-tertiary">
													stdout
												</div>
												<div
													class="whitespace-pre-wrap break-words border-l-2 border-success/50 bg-success/[0.04] py-1 pl-3 text-text"
												>{f.lines.join('\n')}</div>
											</div>
										{:else if f.key === 'stderr'}
											<div class="my-1">
												<div class="text-[10px] uppercase tracking-wider text-text-tertiary">
													stderr
												</div>
												<div
													class="whitespace-pre-wrap break-words border-l-2 border-warning/60 bg-warning/[0.04] py-1 pl-3 text-text"
												>{f.lines.join('\n')}</div>
											</div>
										{:else if f.lines.length === 1}
											<div class="whitespace-pre-wrap break-words">
												<span class="text-text-tertiary">{f.key}:</span>
												<span class="ml-1">{f.lines[0]}</span>
											</div>
										{:else}
											<div>
												<span class="text-text-tertiary">{f.key}:</span>
											</div>
											<div class="whitespace-pre-wrap break-words pl-4">
												{f.lines.join('\n')}
											</div>
										{/if}
									{/each}
								</div>
							{/if}
						</div>
					</div>
				{/each}
			{/if}
		</div>

		{#if autoScrollPaused && lines.length > 0}
			<button
				type="button"
				onclick={jumpToBottom}
				class="absolute bottom-3 right-3 inline-flex items-center gap-1.5 rounded-full border border-border bg-surface-raised px-3 py-1.5 text-xs font-medium text-text shadow-sm transition hover:bg-surface-alt"
			>
				<span>↓ Scroll to bottom{pendingCount > 0 ? ` · ${pendingCount} new` : ''}</span>
			</button>
		{/if}
	</div>
</div>
