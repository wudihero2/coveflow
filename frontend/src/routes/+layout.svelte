<script>
	// Global stylesheet is imported once at the root layout.
	import '../app.css';
	import { Toaster } from 'svelte-sonner';

	import ConfirmDialog from '$lib/components/common/ConfirmDialog.svelte';

	// SvelteKit gives layouts a children snippet for the active child route.
	let { children } = $props();
</script>

<Toaster richColors closeButton position="top-right" />
<ConfirmDialog />

<!-- Root layout renders every route in the app. -->
{@render children()}
