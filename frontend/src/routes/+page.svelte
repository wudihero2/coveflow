<script lang="ts">
	// goto performs client-side navigation without a full page reload.
	import { goto } from '$app/navigation';
	// onMount runs only in the browser, so this redirect does not run during SSR.
	import { onMount } from 'svelte';

	import { auth } from '$lib/stores/auth.svelte';

	// The root page is only a router: authenticated users go to the app,
	// unauthenticated users go to login.
	onMount(() => {
		void redirectFromRoot();
	});

	async function redirectFromRoot() {
		// Try refreshing first so a page reload with a valid refresh cookie stays logged in.
		if (auth.isAuthenticated || (await auth.tryRefresh())) {
			await goto('/home', { replaceState: true });
			return;
		}

		await goto('/user/login', { replaceState: true });
	}
</script>

<svelte:head>
	<title>CoveFlow</title>
</svelte:head>
