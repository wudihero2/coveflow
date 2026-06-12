<!--
  AppShell — Responsive layout with sidebar + main content.

  This component is intentionally structural: callers provide the sidebar and
  page body as snippets, while AppShell only decides where they render.
-->
<script lang="ts">
	// SvelteKit hook that fires after client-side route changes.
	import { afterNavigate } from '$app/navigation';
	// Third-party icon components for mobile drawer + desktop collapse controls.
	import { Menu, X, PanelLeftClose, PanelLeft } from '@lucide/svelte';
	// Snippet is Svelte 5's typed way to receive renderable child content.
	import type { Snippet } from 'svelte';

	import IconButton from '$lib/components/common/IconButton.svelte';

	interface Props {
		sidebar: Snippet;
		children: Snippet;
		brand?: string;
	}

	let { sidebar, children, brand = 'CoveFlow' }: Props = $props();

	// Mobile-only state. Desktop always renders the sidebar directly.
	let drawerOpen = $state(false);

	// Desktop: hide/show the left rail (persisted across reloads).
	let collapsed = $state(false);
	$effect(() => {
		try {
			collapsed = localStorage.getItem('nav-collapsed') === '1';
		} catch {
			/* localStorage unavailable */
		}
	});
	function setCollapsed(v: boolean): void {
		collapsed = v;
		try {
			localStorage.setItem('nav-collapsed', v ? '1' : '0');
		} catch {
			/* ignore */
		}
	}

	// Close the drawer after a sidebar link navigates to a new route.
	afterNavigate(() => {
		drawerOpen = false;
	});
</script>

<!-- Register on window so Escape works even when focus is inside the drawer. -->
<svelte:window onkeydown={(e) => { if (e.key === 'Escape' && drawerOpen) drawerOpen = false; }} />

<div class="flex min-h-svh bg-surface">

	<!-- Desktop left rail (hidden below lg). Collapsible to a thin strip; the
	     strip stays in-flow so it never overlaps page content. State persisted. -->
	{#if collapsed}
		<aside
			class="sticky top-0 hidden h-svh w-10 shrink-0 flex-col items-center border-r border-border bg-surface-raised pt-3 lg:flex"
		>
			<IconButton aria-label="Show sidebar" onclick={() => setCollapsed(false)}>
				<PanelLeft size={18} />
			</IconButton>
		</aside>
	{:else}
		<aside
			class="sticky top-0 hidden h-svh w-60 shrink-0 flex-col border-r border-border bg-surface-raised lg:flex"
		>
			<div class="absolute top-3 right-1.5 z-10">
				<IconButton aria-label="Hide sidebar" onclick={() => setCollapsed(true)}>
					<PanelLeftClose size={16} />
				</IconButton>
			</div>
			{@render sidebar()}
		</aside>
	{/if}

	<!-- Mobile backdrop: rendered only while the drawer is open, click to close. -->
	{#if drawerOpen}
		<div
			class="fixed inset-0 z-40 bg-black/30 lg:hidden"
			aria-hidden="true"
			onclick={() => (drawerOpen = false)}
		></div>
	{/if}

	<!-- `inert` keeps closed drawer links out of keyboard focus and screen readers. -->
	<aside
		inert={!drawerOpen}
		class="fixed inset-y-0 left-0 z-50 flex w-60 flex-col border-r border-border bg-surface-raised transition-transform duration-200 lg:hidden
			{drawerOpen ? 'translate-x-0' : '-translate-x-full'}"
	>
		<div class="absolute right-2 top-3">
			<IconButton aria-label="Close sidebar" onclick={() => (drawerOpen = false)}>
				<X size={18} />
			</IconButton>
		</div>
		{@render sidebar()}
	</aside>

	<!-- Main content grows to fill the remaining width beside the desktop rail. -->
	<div class="min-w-0 flex-1">
		<!-- Mobile-only top bar; desktop navigation lives in the persistent rail. -->
		<div class="sticky top-0 z-30 flex h-12 items-center border-b border-border bg-surface-raised px-3 lg:hidden">
			<IconButton aria-label="Open sidebar" onclick={() => (drawerOpen = true)}>
				<Menu size={20} />
			</IconButton>
			<span class="ml-2 text-sm font-semibold text-text">{brand}</span>
		</div>
		<main class="min-w-0">
			{@render children()}
		</main>
	</div>
</div>
