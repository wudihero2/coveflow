<!--
  AppSidebar renders the sidebar content for authenticated pages.

  AppShell decides where this sidebar appears. This component only renders the
  brand, workspace picker, navigation links, admin links, and user actions.
-->
<script lang="ts">
	// SvelteKit route state; used here to know which sidebar link is active.
	import { page } from '$app/state';
	// Third-party icon component from lucide-svelte for the logout action.
	import { KeyRound, LogOut } from '@lucide/svelte';
	// Svelte's component type lets nav items carry icon components safely.
	import type { Component } from 'svelte';

	import CoveFlowLogo from '$lib/components/common/CoveFlowLogo.svelte';
	import IconButton from '$lib/components/common/IconButton.svelte';

	import WorkspaceSwitcher from './WorkspaceSwitcher.svelte';
	import TimezonePicker from '$lib/components/common/TimezonePicker.svelte';

	// Each nav item owns its route, label, and icon component.
	interface NavItem {
		href: string;
		label: string;
		icon: Component;
	}

	interface WorkspaceOption {
		id: string;
		name: string;
	}

	interface Props {
		navItems: NavItem[];
		adminItems?: NavItem[];
		email: string | null;
		workspaces?: WorkspaceOption[];
		currentWorkspace?: string;
		onLogout: () => void;
		onSwitchWorkspace?: (id: string) => void;
	}

	// The authenticated layout owns the data and handlers; this component only displays them.
	let {
		navItems,
		adminItems = [],
		email,
		workspaces = [],
		currentWorkspace = '',
		onLogout,
		onSwitchWorkspace
	}: Props = $props();

	// Highlight both exact matches and nested routes, e.g. /admin/members/123.
	function isActive(href: string): boolean {
		const path = page.url.pathname;
		return path === href || path.startsWith(`${href}/`);
	}
</script>

<nav class="flex h-full flex-col">
	<!-- Brand header stays at the top of the rail or mobile drawer. -->
	<a href="/home" class="flex items-center gap-3 border-b border-border px-5 py-4 transition hover:bg-surface-alt">
		<CoveFlowLogo height={32} />
		<span class="text-lg font-semibold tracking-tight text-text">coveflow</span>
	</a>

	<!-- Workspace indicator: shows name when single, dropdown when multiple. -->
	<div class="pt-2">
		<WorkspaceSwitcher
			{workspaces}
			current={currentWorkspace}
			onSwitch={onSwitchWorkspace}
		/>
	</div>

	<!-- Main navigation is visible to every authenticated user. -->
	<ul class="mt-3 flex flex-col gap-0.5 px-3">
		{#each navItems as item (item.href)}
			{@const active = isActive(item.href)}
			<li>
				<a
					href={item.href}
					aria-current={active ? 'page' : undefined}
					class="flex items-center gap-3 rounded-md px-2.5 py-2 text-sm transition
						{active
							? 'bg-accent-subtle font-medium text-accent'
							: 'text-text-secondary hover:bg-surface-alt hover:text-text'}"
				>
					<item.icon size={18} />
					{item.label}
				</a>
			</li>
		{/each}
	</ul>

	<!-- Admin navigation is rendered only when the layout passes admin items. -->
	{#if adminItems.length > 0}
		<div class="mx-5 mt-4 border-t border-border-subtle"></div>
		<p class="mt-3 px-5 text-[11px] font-semibold uppercase tracking-wider text-text-tertiary">
			Admin
		</p>
		<ul class="mt-1 flex flex-col gap-0.5 px-3">
			{#each adminItems as item (item.href)}
				{@const active = isActive(item.href)}
				<li>
					<a
						href={item.href}
						aria-current={active ? 'page' : undefined}
						class="flex items-center gap-3 rounded-md px-2.5 py-2 text-sm transition
							{active
								? 'bg-accent-subtle font-medium text-accent'
								: 'text-text-secondary hover:bg-surface-alt hover:text-text'}"
					>
						<item.icon size={18} />
						{item.label}
					</a>
				</li>
			{/each}
		</ul>
	{/if}

	<!-- Fill remaining height so the user footer stays pinned to the bottom. -->
	<div class="flex-1"></div>

	<!-- Personal settings: API tokens (account-global, used to call webhooks). -->
	<a
		href="/settings/tokens"
		class="flex items-center gap-2.5 border-t border-border px-5 py-2 text-sm text-text-secondary transition-colors hover:text-text"
	>
		<KeyRound size={16} class="shrink-0" />
		API Tokens
	</a>

	<!-- Display timezone (how times render app-wide; not the schedule's own tz). -->
	<div class="border-t border-border px-5 py-2">
		<TimezonePicker />
	</div>

	<!-- User identity and logout action. Logout behavior is owned by the layout. -->
	<div class="border-t border-border px-3 py-3">
		<div class="flex items-center gap-2 px-2">
			<div class="flex h-7 w-7 shrink-0 items-center justify-center rounded-full bg-surface-sunken text-xs font-medium text-text-secondary">
				{email ? email[0].toUpperCase() : '?'}
			</div>
			<span class="min-w-0 flex-1 truncate text-sm text-text-secondary" title={email ?? ''}>
				{email ?? 'Unknown'}
			</span>
			<IconButton variant="danger" title="Log out" aria-label="Log out" onclick={onLogout}>
				<LogOut size={16} />
			</IconButton>
		</div>
	</div>
</nav>
