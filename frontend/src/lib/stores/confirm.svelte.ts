import { goto } from '$app/navigation';
import type { BeforeNavigate } from '@sveltejs/kit';

export interface ConfirmOptions {
	title: string;
	/** Body text. Newlines are preserved (rendered with whitespace-pre-line). */
	message?: string;
	confirmLabel?: string;
	cancelLabel?: string;
	variant?: 'danger' | 'primary';
}

interface PendingConfirm extends ConfirmOptions {
	resolve: (value: boolean) => void;
}

/**
 * Backing store for the app-wide confirmation dialog. A single `<ConfirmDialog>`
 * host (mounted in the root layout) renders `current` through the themed Modal,
 * so confirmations match the rest of the UI instead of the browser's native,
 * unstyled `window.confirm()`.
 */
class ConfirmStore {
	current = $state<PendingConfirm | null>(null);

	ask(options: ConfirmOptions): Promise<boolean> {
		// A new prompt supersedes any still-open one (resolve it as cancelled).
		this.current?.resolve(false);
		return new Promise<boolean>((resolve) => {
			this.current = { ...options, resolve };
		});
	}

	settle(value: boolean): void {
		const pending = this.current;
		this.current = null;
		pending?.resolve(value);
	}
}

export const confirmStore = new ConfirmStore();

/** Promise-based replacement for `window.confirm()`, rendered via the app Modal. */
export function confirmDialog(options: ConfirmOptions): Promise<boolean> {
	return confirmStore.ask(options);
}

// Re-entrancy guard for confirmNavigation: when the user confirms a leave we
// re-issue the navigation, which fires beforeNavigate again — this lets that
// second pass through without re-prompting.
let bypassNextNavigation = false;

/**
 * Drop-in confirmation for `beforeNavigate` guards. `beforeNavigate` is
 * synchronous (cancel must happen now), but our Modal is async — so we cancel
 * immediately, ask, and re-issue the navigation on confirm. The caller is
 * responsible for its own skip conditions (not dirty, full-page unload, etc.)
 * before calling this; full-page unloads can only be guarded by the browser's
 * native onbeforeunload prompt.
 */
export function confirmNavigation(navigation: BeforeNavigate, options: ConfirmOptions): void {
	// Full-page unload / external navigation can't be guarded by an async modal —
	// the browser's native onbeforeunload owns it. Returning (without cancelling)
	// also avoids silently blocking external links (where navigation.to is null).
	if (navigation.willUnload) return;
	if (bypassNextNavigation) {
		bypassNextNavigation = false;
		return;
	}
	const to = navigation.to?.url;
	navigation.cancel();
	if (!to) return;
	void confirmDialog(options).then((ok) => {
		if (!ok) return;
		bypassNextNavigation = true;
		void goto(to.pathname + to.search + to.hash);
	});
}
