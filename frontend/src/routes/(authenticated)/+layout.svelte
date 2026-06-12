<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import {
		FileCode,
		FolderOpen,
		Home,
		KeyRound,
		Play,
		Server,
		Settings,
		Users,
		UsersRound
	} from '@lucide/svelte';
	import { onMount, setContext } from 'svelte';

	import AppShell from '$lib/components/layout/AppShell.svelte';
	import AppSidebar from '$lib/components/layout/AppSidebar.svelte';
	import { useWorkspaceLoader } from '$lib/services/workspace-loader.svelte';
	import { auth } from '$lib/stores/auth.svelte';
	import { workspace } from '$lib/stores/workspace.svelte';

	// SvelteKit passes the matched child page as `children`.
	// Rename it here so it is not confused with AppShell's own children below.
	let { children: pageContent } = $props();

	// ---------------------------------------------------------------------------
	// 1. Auth Guard
	//    - Checks token on mount; tries refresh if expired; redirects if invalid.
	//    - `ready` gates rendering: nothing shows until auth is confirmed.
	//    - The $effect monitors for mid-session token expiry (e.g. refresh failed).
	// ---------------------------------------------------------------------------

	let ready = $state(false);
	let loginRedirect = $derived.by(() => {
		const params = new URLSearchParams({
			next: page.url.pathname + page.url.search
		});
		if (workspace.lastId !== 'default') {
			params.set('workspace', workspace.lastId);
		}
		return `/user/login?${params.toString()}`;
	});

	onMount(() => {
		void guard();
	});

	async function guard() {
		if (auth.isAuthenticated) {
			ready = true;
			return;
		}
		const refreshed = await auth.tryRefresh();
		if (refreshed) {
			ready = true;
		} else {
			await goto(loginRedirect, { replaceState: true });
		}
	}

	$effect(() => {
		if (ready && !auth.isAuthenticated) {
			void goto(loginRedirect, { replaceState: true });
		}
	});

	// ---------------------------------------------------------------------------
	// 2. Role Management
	//    - Fetches user's role in the current workspace via GET /me.
	//    - Three states (role / roleLoaded / roleError) let the admin guard in
	//      admin/+layout.svelte distinguish "still loading" / "fetch failed" /
	//      "confirmed non-admin" — so it only redirects on confirmed non-admin.
	//    - Re-fetches whenever workspace changes; the useWorkspaceLoader stale
	//      guard prevents race conditions on rapid switching.
	// ---------------------------------------------------------------------------

	const roleLoader = useWorkspaceLoader((ws) => ws.getMe(), {
		enabled: () => ready
	});

	let meData = $derived(
		roleLoader.data as { role?: string; is_instance_admin?: boolean } | null
	);
	let role = $derived(meData?.role ?? null);
	let roleLoaded = $derived(!roleLoader.loading);
	let roleError = $derived(!!roleLoader.error);
	let isAdmin = $derived(role === 'admin');

	// Instance-admin is instance-wide, but it arrives via the workspace-scoped
	// /me which nulls its data at the start of every workspace switch. Cache the
	// last known value so the Cluster nav item doesn't flicker (and the admin
	// guard doesn't briefly redirect) mid-switch.
	let isInstanceAdmin = $state(false);
	$effect(() => {
		if (meData) isInstanceAdmin = meData.is_instance_admin ?? false;
	});

	setContext('auth:role', () => ({ role, roleLoaded, roleError, isInstanceAdmin }));

	// ---------------------------------------------------------------------------
	// 3. Sidebar Navigation
	//    - navItems: visible to all users.
	//    - adminItems: only passed to sidebar when isAdmin is true.
	// ---------------------------------------------------------------------------

	const navItems = [
		{ href: '/home', label: 'Home', icon: Home },
		// Scripts + flows share one explorer (the (workspace) group layout).
		{ href: '/scripts', label: 'Files', icon: FileCode },
		{ href: '/runs', label: 'Runs', icon: Play },
		{ href: '/secrets', label: 'Secrets', icon: KeyRound }
	];

	// Workspace-admin items (only when the user is a workspace admin) plus the
	// Cluster dashboard, which is instance-admin only and shown independently.
	const workspaceAdminItems = [
		{ href: '/admin/members', label: 'Members', icon: Users },
		{ href: '/admin/teams', label: 'Teams', icon: UsersRound },
		{ href: '/service-logs', label: 'Service Logs', icon: Settings }
	];
	const clusterItem = { href: '/admin/cluster', label: 'Cluster', icon: Server };

	let adminItems = $derived([
		...(isAdmin ? workspaceAdminItems : []),
		...(isInstanceAdmin ? [clusterItem] : [])
	]);

	// ---------------------------------------------------------------------------
	// 4. Event Handlers
	// ---------------------------------------------------------------------------

	async function handleLogout() {
		ready = false;
		await auth.logout();
		await goto('/user/login');
	}

	function handleSwitchWorkspace(id: string) {
		workspace.switch(id);
	}
</script>

{#if ready}
	<!-- AppShell owns the responsive frame; this layout supplies its content. -->
	<AppShell>
		<!-- Named snippet: AppShell decides where the sidebar is rendered. -->
		{#snippet sidebar()}
			<AppSidebar
				{navItems}
				{adminItems}
				email={auth.email}
				workspaces={workspace.list.map((w) => ({ id: w.id, name: w.name }))}
				currentWorkspace={workspace.id}
				onLogout={handleLogout}
				onSwitchWorkspace={handleSwitchWorkspace}
			/>
		{/snippet}

		<!-- Current child route, rendered inside AppShell's main content area. -->
		{@render pageContent()}
	</AppShell>
{/if}
