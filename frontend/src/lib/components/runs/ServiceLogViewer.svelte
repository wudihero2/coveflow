<script lang="ts">
	import {
		TriangleAlert,
		CircleDot,
		Copy,
		Download,
		LoaderCircle,
		ChevronRight,
		Pause,
		Play,
		Search,
		X
	} from '@lucide/svelte';
	import { SvelteSet } from 'svelte/reactivity';
	import IconButton from '$lib/components/common/IconButton.svelte';
	import type { LogLevelName } from '$lib/services/api';
	import { pollServiceLogs } from '$lib/services/polling';
	import { toastError, toastInfo, toastSuccess } from '$lib/toast';
	import { displayTz } from '$lib/stores/timezone.svelte';
	import { formatAbsolute } from '$lib/utils/format-time';
	import type { LogEntry } from '$lib/types';
	import { appendManyCapped } from './log/buffer';
	import {
		formatEntryFields,
		formatTimestamp,
		levelBorderClass,
		levelClass,
		levelLabel
	} from './log/format';

	interface IdentifiedServiceLogEntry extends LogEntry {
		_id: number;
		service: string;
		instance_id: string;
	}

	type Status = 'connecting' | 'streaming' | 'error';

	interface Props {
		workspaceId: string;
		service?: string;
		instance?: string;
		minLevel?: number;
		sinceMs?: number;
		untilMs?: number;
		onChunkReceived?: (service: string, instanceId: string) => void;
		class?: string;
	}

	let {
		workspaceId,
		service,
		instance,
		minLevel = 1,
		sinceMs,
		untilMs,
		onChunkReceived,
		class: className = ''
	}: Props = $props();

	let lines = $state.raw<readonly IdentifiedServiceLogEntry[]>([]);
	let status = $state<Status>('connecting');

	// Manual pause: completely freezes `lines`; new entries go to pauseBuffer.
	let manualPaused = $state(false);
	// Non-reactive buffer — no proxy overhead, never drives rendering directly.
	const pauseBuffer: IdentifiedServiceLogEntry[] = [];
	let pauseBufferCount = $state(0);

	// Scroll-based pause: lines still grow, floating button shows pending count.
	let autoScrollPaused = $state(false);
	let pendingCount = $state(0);

	let container = $state<HTMLDivElement | undefined>();
	const expandedIds = new SvelteSet<number>();
	let nextEntryId = 0;


	// Search filter — client-side only, never restarts polling.
	let searchQuery = $state('');

	const displayLines = $derived.by(() => {
		let result: readonly IdentifiedServiceLogEntry[] = lines;

		const q = searchQuery.trim().toLowerCase();
		if (q) {
			result = result.filter(
				(l) =>
					l.msg.toLowerCase().includes(q) ||
					l.service.toLowerCase().includes(q) ||
					l.instance_id.toLowerCase().includes(q)
			);
		}

		return result;
	});

	function splitHighlight(text: string, query: string): Array<{ text: string; match: boolean }> {
		if (!query) return [{ text, match: false }];
		const escaped = query.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
		const parts = text.split(new RegExp(`(${escaped})`, 'gi'));
		return parts.map((part, i) => ({ text: part, match: i % 2 === 1 }));
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

	function serviceEntryToPlainText(entry: IdentifiedServiceLogEntry): string {
		const ts = formatTimestamp(entry.ts);
		const lvl = levelLabel(entry.level).padEnd(5, ' ');
		const svc = entry.service.padEnd(12, ' ');
		const inst = entry.instance_id.padEnd(16, ' ');
		const header = `${ts}  ${svc}  ${inst}  ${lvl}  ${entry.msg}`;

		const fields = formatEntryFields(entry.fields);
		if (fields.length === 0) return header;

		const indent = '    ';
		const fieldLines: string[] = [];
		for (const f of fields) {
			const isOutput = f.key === 'stdout' || f.key === 'stderr';
			if (isOutput) {
				for (const line of f.lines) fieldLines.push(`${indent}${line}`);
			} else if (f.lines.length === 1) {
				fieldLines.push(`${indent}${f.key}: ${f.lines[0]}`);
			} else {
				fieldLines.push(`${indent}${f.key}:`);
				for (const line of f.lines) fieldLines.push(`${indent}  ${line}`);
			}
		}
		return `${header}\n${fieldLines.join('\n')}`;
	}

	function serviceEntriesToPlainText(entries: readonly IdentifiedServiceLogEntry[]): string {
		return entries.map(serviceEntryToPlainText).join('\n');
	}

	function toggleExpanded(id: number): void {
		if (expandedIds.has(id)) expandedIds.delete(id);
		else expandedIds.add(id);
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

	let wasAtBottomBeforePause = false;

	function togglePause(): void {
		if (manualPaused) {
			const flushed = pauseBuffer.length;
			if (flushed > 0) {
				lines = appendManyCapped(lines, pauseBuffer);
				pauseBuffer.length = 0;
			}
			pauseBufferCount = 0;
			manualPaused = false;
			if (wasAtBottomBeforePause) {
				autoScrollPaused = false;
				pendingCount = 0;
				scrollToBottom();
			} else if (flushed > 0) {
				pendingCount += flushed;
			}
		} else {
			wasAtBottomBeforePause = !autoScrollPaused;
			manualPaused = true;
		}
	}

	$effect(() => {
		lines = [];
		pendingCount = 0;
		pauseBuffer.length = 0;
		pauseBufferCount = 0;
		autoScrollPaused = false;
		manualPaused = false;
		expandedIds.clear();
		status = 'connecting';

		let toastedError = false;

		const cleanup = pollServiceLogs(workspaceId, {
			service,
			instance,
			level: levelToName(minLevel),
			sinceMs,
			onOpen() {
				const recoveredFromError = status === 'error';
				status = 'streaming';
				if (recoveredFromError && toastedError) {
					toastInfo('Reconnected');
				}
				toastedError = false;
			},
			onLog(chunk) {
				if (status === 'connecting' || status === 'error') status = 'streaming';
				onChunkReceived?.(chunk.service, chunk.instance_id);
				let identified: IdentifiedServiceLogEntry[] = chunk.entries.map((e) => ({
					...e,
					_id: nextEntryId++,
					service: chunk.service,
					instance_id: chunk.instance_id
				}));
				if (untilMs) {
					identified = identified.filter((e) => new Date(e.ts).getTime() <= untilMs);
					if (identified.length === 0) return;
				}
				if (manualPaused) {
					// Completely freeze the view — buffer until user resumes.
					for (const entry of identified) pauseBuffer.push(entry);
					pauseBufferCount += identified.length;
				} else {
					lines = appendManyCapped(lines, identified);
					if (autoScrollPaused) {
						pendingCount += identified.length;
					} else {
						scrollToBottom();
					}
				}
			},
			onError(message) {
				status = 'error';
				if (!toastedError) {
					toastedError = true;
					toastError(message);
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
		if (manualPaused) {
			return {
				icon: Pause,
				label: `Paused · ${lines.length.toLocaleString()} lines`,
				color: 'text-text-secondary',
				spin: false,
				pulse: false
			};
		}
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
					label: `Streaming · ${lines.length.toLocaleString()} lines`,
					color: 'text-info',
					spin: false,
					pulse: true
				};
			default:
				return {
					icon: TriangleAlert,
					label: `Connection error · ${lines.length.toLocaleString()} lines`,
					color: 'text-error',
					spin: false,
					pulse: false
				};
		}
	});

	async function copyAll(): Promise<void> {
		try {
			const src = displayLines.length < lines.length ? displayLines : lines;
			await navigator.clipboard.writeText(serviceEntriesToPlainText(src));
			toastSuccess(`Copied ${src.length.toLocaleString()} lines`);
		} catch (e) {
			toastError(e instanceof Error ? e.message : 'Copy failed');
		}
	}

	function downloadTxt(): void {
		const src = displayLines.length < lines.length ? displayLines : lines;
		const blob = new Blob([serviceEntriesToPlainText(src)], { type: 'text/plain' });
		const url = URL.createObjectURL(blob);
		const a = document.createElement('a');
		a.href = url;
		a.download = 'coveflow-service-logs.txt';
		document.body.appendChild(a);
		a.click();
		document.body.removeChild(a);
		URL.revokeObjectURL(url);
	}

	const liveDescription = $derived(`Service log stream ${status}, ${lines.length} lines`);
</script>

<!--
	Highlight snippet at template root so it is stable across conditional
	re-renders and does not interfere with the search input's focus state.
-->
{#snippet highlightText(text: string)}
	{#if searchQuery.trim()}
		{#each splitHighlight(text, searchQuery.trim()) as part, i (i)}
			{#if part.match}
				<mark class="rounded bg-warning/40 px-px text-text">{part.text}</mark>
			{:else}
				{part.text}
			{/if}
		{/each}
	{:else}
		{text}
	{/if}
{/snippet}

<div class="flex min-h-0 flex-col {className}">
	<!-- Row 1: status + manual-pause chip + controls -->
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

		{#if !manualPaused && autoScrollPaused && pendingCount > 0}
			<span class="rounded-md bg-surface-sunken px-2 py-0.5 text-[11px] text-text-tertiary">
				↓ {pendingCount.toLocaleString()} new
			</span>
		{/if}

		<div class="flex-1"></div>

		<IconButton
			aria-label={manualPaused ? 'Resume stream' : 'Pause stream'}
			title={manualPaused
				? pauseBufferCount > 0
					? `Resume · flush ${pauseBufferCount} buffered`
					: 'Resume'
				: 'Pause'}
			onclick={togglePause}
		>
			{#if manualPaused}
				<Play size={14} class="text-accent" />
			{:else}
				<Pause size={14} />
			{/if}
		</IconButton>
		<IconButton
			aria-label="Copy visible logs"
			title="Copy visible logs"
			onclick={copyAll}
			disabled={displayLines.length === 0}
		>
			<Copy size={14} />
		</IconButton>
		<IconButton
			aria-label="Download log as .txt"
			title="Download .txt"
			onclick={downloadTxt}
			disabled={displayLines.length === 0}
		>
			<Download size={14} />
		</IconButton>
	</div>

	<!-- Row 2: search + time window + match count -->
	<div class="flex items-center gap-2 border-b border-border bg-surface-alt px-3 py-1.5">
		<!-- Search input — uses oninput (not bind:value) to avoid focus-reset
		     issues when the parent component re-renders on every polling tick. -->
		<div class="relative min-w-0 flex-1">
			<Search
				size={12}
				class="pointer-events-none absolute left-2.5 top-1/2 -translate-y-1/2 text-text-tertiary"
			/>
			<input
				type="text"
				value={searchQuery}
				oninput={(e) => (searchQuery = e.currentTarget.value)}
				placeholder="Search logs…"
				class="h-7 w-full rounded border border-border bg-surface pl-7 pr-6 text-xs text-text outline-none placeholder:text-text-tertiary focus:border-accent focus:ring-1 focus:ring-accent"
			/>
			{#if searchQuery}
				<button
					type="button"
					onclick={() => (searchQuery = '')}
					class="absolute right-1.5 top-1/2 -translate-y-1/2 rounded p-0.5 text-text-tertiary hover:bg-surface-alt hover:text-text"
					aria-label="Clear search"
				>
					<X size={11} />
				</button>
			{/if}
		</div>


		{#if searchQuery.trim()}
			<span class="shrink-0 text-[11px] text-text-tertiary">
				{displayLines.length.toLocaleString()} / {lines.length.toLocaleString()}
			</span>
		{/if}
	</div>

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
			{#if lines.length === 0 && status === 'connecting'}
				<div class="flex h-full flex-col items-center justify-center gap-2 text-text-tertiary">
					<LoaderCircle size={20} class="animate-spin" />
					<p class="text-sm">Connecting to service logs…</p>
				</div>
			{:else if lines.length === 0 && status === 'streaming'}
				<div class="flex h-full flex-col items-center justify-center gap-2 text-text-tertiary">
					<p class="text-sm">Waiting for logs…</p>
				</div>
			{:else if displayLines.length === 0}
				<div class="flex h-full flex-col items-center justify-center gap-2 text-text-tertiary">
					<Search size={20} />
					<p class="text-sm">No logs match the current filter.</p>
					<p class="text-xs">{lines.length.toLocaleString()} total lines in buffer.</p>
				</div>
			{:else}
				{#each displayLines as entry (entry._id)}
					{@const fields = formatEntryFields(entry.fields)}
					{@const hasFields = fields.length > 0}
					{@const isExpanded = expandedIds.has(entry._id)}
					{@const fieldsId = `entry-${entry._id}-fields`}
					<div
						class="my-0.5 flex items-start gap-1 border-l-2 {levelBorderClass(
							entry.level
						)} pl-2"
					>
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
							<span class="mt-0.5 inline-block w-3 shrink-0"></span>
						{/if}
						<div class="min-w-0 flex-1">
							<div class="flex flex-wrap items-baseline gap-x-2 whitespace-pre-wrap break-all">
								<span class="shrink-0 text-text-tertiary">{formatAbsolute(entry.ts, displayTz.value)}</span>
								<span class="w-16 shrink-0 truncate text-text-tertiary">{@render highlightText(entry.service)}</span>
								<span class="w-24 shrink-0 truncate text-text-tertiary"
									>{@render highlightText(entry.instance_id)}</span
								>
								<span class="shrink-0 {levelClass(entry.level)}"
									>{levelLabel(entry.level)}</span
								>
								<span class="text-text">{@render highlightText(entry.msg)}</span>
							</div>
							{#if hasFields && isExpanded}
								<div
									id={fieldsId}
									class="ml-3 mt-0.5 border-l border-border pl-3 text-text-secondary"
								>
									{#each fields as f (f.key)}
										{#if f.key === 'stdout'}
											<div class="my-1">
												<div
													class="text-[10px] uppercase tracking-wider text-text-tertiary"
												>
													stdout
												</div>
												<div
													class="whitespace-pre-wrap break-words border-l-2 border-success/50 bg-success/[0.04] py-1 pl-3 text-text"
												>{f.lines.join('\n')}</div>
											</div>
										{:else if f.key === 'stderr'}
											<div class="my-1">
												<div
													class="text-[10px] uppercase tracking-wider text-text-tertiary"
												>
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

		{#if !manualPaused && autoScrollPaused && pendingCount > 0}
			<button
				type="button"
				onclick={jumpToBottom}
				class="absolute bottom-3 right-3 inline-flex items-center gap-1.5 rounded-full border border-border bg-surface-raised px-3 py-1.5 text-xs font-medium text-text shadow-sm transition hover:bg-surface-alt"
			>
				<span>↓ Scroll to bottom · {pendingCount.toLocaleString()} new</span>
			</button>
		{/if}
	</div>
</div>
