<script lang="ts">
	import { onMount, type Snippet } from 'svelte';

	interface Props {
		primary: Snippet;
		secondary: Snippet;
		defaultPercent?: number;
		minPrimaryPx?: number;
		minSecondaryPx?: number;
		primaryId?: string;
		secondaryId?: string;
		storageKey?: string;
		ariaLabel?: string;
		class?: string;
	}

	let {
		primary,
		secondary,
		defaultPercent = 60,
		minPrimaryPx = 420,
		minSecondaryPx = 320,
		primaryId,
		secondaryId,
		storageKey,
		ariaLabel = 'Resize panels',
		class: className = ''
	}: Props = $props();

	const generatedId = $props.id();
	const HANDLE_WIDTH_PX = 9;
	const KEYBOARD_STEP = 1;
	const KEYBOARD_LARGE_STEP = 5;
	const DEFAULT_MIN_PERCENT = 10;
	const DEFAULT_MAX_PERCENT = 90;
	const RESIZE_KEYS = new Set(['ArrowLeft', 'ArrowRight', 'Home', 'End']);

	let containerWidth = $state(0);
	let percent = $state<number | undefined>();
	let dragging = $state(false);
	let previousBodyCursor: string | undefined;
	let previousBodyUserSelect: string | undefined;
	let primaryPanelId = $derived(primaryId ?? `${generatedId}-primary`);
	let secondaryPanelId = $derived(secondaryId ?? `${generatedId}-secondary`);
	let controlledPanelIds = $derived(`${primaryPanelId} ${secondaryPanelId}`);

	function clamp(value: number, min: number, max: number): number {
		return Math.min(max, Math.max(min, value));
	}

	let percentBounds = $derived.by(() => {
		const width = containerWidth - HANDLE_WIDTH_PX;
		if (width <= 0) {
			return { min: DEFAULT_MIN_PERCENT, max: DEFAULT_MAX_PERCENT };
		}

		const min = (minPrimaryPx / width) * 100;
		const max = 100 - (minSecondaryPx / width) * 100;
		if (min > max) {
			return { min: DEFAULT_MIN_PERCENT, max: DEFAULT_MAX_PERCENT };
		}

		return {
			min: clamp(min, DEFAULT_MIN_PERCENT, DEFAULT_MAX_PERCENT),
			max: clamp(max, DEFAULT_MIN_PERCENT, DEFAULT_MAX_PERCENT)
		};
	});

	let effectivePercent = $derived(
		clamp(percent ?? defaultPercent, percentBounds.min, percentBounds.max)
	);
	let afterPercent = $derived(100 - effectivePercent);
	let roundedPercent = $derived(Math.round(effectivePercent));
	let gridTemplate = $derived(
		`minmax(0, calc(${effectivePercent}% - ${HANDLE_WIDTH_PX / 2}px)) ${HANDLE_WIDTH_PX}px minmax(0, calc(${afterPercent}% - ${HANDLE_WIDTH_PX / 2}px))`
	);

	function persistPercent(value: number): void {
		if (!storageKey) return;
		try {
			window.localStorage.setItem(storageKey, value.toFixed(2));
		} catch {
			// Ignore persistence failures; the splitter remains fully usable.
		}
	}

	function setPercent(value: number, persist = true): void {
		const next = clamp(value, percentBounds.min, percentBounds.max);
		percent = next;
		if (persist) {
			persistPercent(next);
		}
	}

	function setPercentFromClientX(clientX: number, target: HTMLElement): void {
		const rect = target.getBoundingClientRect();
		if (rect.width <= 0) return;
		setPercent(((clientX - rect.left) / rect.width) * 100, false);
	}

	function isResizeKey(key: string): boolean {
		return RESIZE_KEYS.has(key);
	}

	function splitContainerFromTarget(target: EventTarget | null): HTMLElement | null {
		if (!(target instanceof HTMLElement)) return null;
		return target.parentElement instanceof HTMLElement ? target.parentElement : null;
	}

	function handlePointerDown(event: PointerEvent): void {
		if (event.button !== 0) return;
		const target = event.currentTarget;
		if (!(target instanceof HTMLElement)) return;

		dragging = true;
		lockBodySelection();
		target.setPointerCapture(event.pointerId);
		event.preventDefault();
	}

	function handlePointerMove(event: PointerEvent): void {
		if (!dragging) return;
		const container = splitContainerFromTarget(event.currentTarget);
		if (!container) return;

		setPercentFromClientX(event.clientX, container);
	}

	function handlePointerUp(event: PointerEvent): void {
		if (!dragging) return;
		const target = event.currentTarget;
		if (target instanceof HTMLElement && target.hasPointerCapture(event.pointerId)) {
			target.releasePointerCapture(event.pointerId);
		}
		dragging = false;
		unlockBodySelection();
		persistPercent(effectivePercent);
	}

	function handleKeydown(event: KeyboardEvent): void {
		const step = event.shiftKey ? KEYBOARD_LARGE_STEP : KEYBOARD_STEP;
		switch (event.key) {
			case 'ArrowLeft':
				setPercent(effectivePercent - step, false);
				break;
			case 'ArrowRight':
				setPercent(effectivePercent + step, false);
				break;
			case 'Home':
				setPercent(percentBounds.min, false);
				break;
			case 'End':
				setPercent(percentBounds.max, false);
				break;
			default:
				return;
		}
		event.preventDefault();
	}

	function handleKeyup(event: KeyboardEvent): void {
		if (!isResizeKey(event.key)) return;
		persistPercent(effectivePercent);
	}

	function resetSplit(): void {
		setPercent(defaultPercent);
	}

	function lockBodySelection(): void {
		if (previousBodyCursor !== undefined || previousBodyUserSelect !== undefined) return;
		previousBodyCursor = document.body.style.cursor;
		previousBodyUserSelect = document.body.style.userSelect;
		document.body.style.cursor = 'col-resize';
		document.body.style.userSelect = 'none';
	}

	function unlockBodySelection(): void {
		if (previousBodyCursor === undefined || previousBodyUserSelect === undefined) return;
		document.body.style.cursor = previousBodyCursor;
		document.body.style.userSelect = previousBodyUserSelect;
		previousBodyCursor = undefined;
		previousBodyUserSelect = undefined;
	}

	onMount(() => {
		if (!storageKey) return unlockBodySelection;
		try {
			const stored = window.localStorage.getItem(storageKey);
			const value = stored ? Number(stored) : NaN;
			if (Number.isFinite(value)) {
				setPercent(value, false);
			}
		} catch {
			// localStorage can be unavailable in private browsing or restricted contexts.
		}

		return () => {
			unlockBodySelection();
		};
	});
</script>

<div
	bind:clientWidth={containerWidth}
	class="grid min-h-0 flex-1 grid-cols-1 grid-rows-2 overflow-hidden md:grid-rows-1 md:[grid-template-columns:var(--split-grid)] {className}"
	style:--split-grid={gridTemplate}
>
	<div id={primaryPanelId} class="flex min-h-0 min-w-0 flex-col overflow-hidden">
		{@render primary()}
	</div>

	<!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
	<div
		role="separator"
		tabindex="0"
		aria-label={ariaLabel}
		aria-orientation="vertical"
		aria-valuemin={Math.round(percentBounds.min)}
		aria-valuemax={Math.round(percentBounds.max)}
		aria-valuenow={roundedPercent}
		aria-valuetext={`${roundedPercent}%`}
		aria-controls={controlledPanelIds}
		title="Drag to resize panels. Use arrow keys for fine control. Double-click to reset."
		class="group hidden min-h-0 cursor-col-resize touch-none items-center justify-center bg-border outline-none transition hover:bg-accent/25 focus-visible:bg-accent/25 focus-visible:ring-2 focus-visible:ring-accent md:flex {dragging ? 'bg-accent/30' : ''}"
		onpointerdown={handlePointerDown}
		onpointermove={handlePointerMove}
		onpointerup={handlePointerUp}
		onpointercancel={handlePointerUp}
		onlostpointercapture={handlePointerUp}
		onkeydown={handleKeydown}
		onkeyup={handleKeyup}
		ondblclick={resetSplit}
	>
		<span
			class="h-12 w-px rounded-full bg-border-strong transition group-hover:bg-accent group-focus-visible:bg-accent"
		></span>
	</div>

	<div
		id={secondaryPanelId}
		class="flex min-h-0 min-w-0 flex-col overflow-hidden border-t border-border md:border-t-0"
	>
		{@render secondary()}
	</div>
</div>
