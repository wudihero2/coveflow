<script lang="ts">
	// Admin guard redirects confirmed non-admin users back to the main app.
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	// getContext reads role state provided by the authenticated parent layout.
	import { getContext } from 'svelte';
	// children is the nested admin page content.
	import type { Snippet } from 'svelte';
	import { isInstanceAdminPath } from '$lib/auth/admin-routes';
	import type { WorkspaceRole } from '$lib/types';

	let { children }: { children: Snippet } = $props();

	const getAuthRole = getContext<
		() => {
			role: WorkspaceRole | null;
			roleLoaded: boolean;
			roleError: boolean;
			isInstanceAdmin: boolean;
		}
	>('auth:role');

	// Single route guard for all of /admin. Instance-admin routes (e.g. the cluster
	// dashboard) require the instance-admin flag regardless of workspace role; every
	// other /admin page requires a workspace-admin role. Enforcing the rule here
	// means pages underneath don't re-implement their own redirects.
	let allowed = $derived.by(() => {
		const { role, isInstanceAdmin } = getAuthRole();
		if (isInstanceAdminPath(page.url.pathname)) return isInstanceAdmin;
		return role === 'admin';
	});

	$effect(() => {
		const { roleLoaded, roleError } = getAuthRole();
		if (roleLoaded && !roleError && !allowed) {
			void goto('/scripts', { replaceState: true });
		}
	});
</script>

{#if allowed}
	{@render children()}
{/if}
