<script lang="ts">
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { auth } from '$lib/stores/auth.svelte';

	let { children } = $props();

	// Public auth pages — if the user is already signed in, bounce them to
	// the app instead of letting them see the login form. Otherwise they
	// can re-authenticate freely.
	onMount(() => {
		if (auth.isAuthenticated) {
			void goto('/scripts');
		}
	});
</script>

<!--
	Centered card frame for /user/login and /user/signup. The card itself
	(border, padding, brand header) lives in AuthCard.svelte; this layout
	just provides the page-level background and centering.
-->
<main class="grid min-h-svh place-items-center bg-surface px-5 py-8 text-text">
	{@render children()}
</main>
