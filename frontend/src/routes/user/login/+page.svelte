<script lang="ts">
	// Route page only renders the form. Centering and bg live in user/+layout.svelte,
	// the card frame lives in AuthCard.svelte, and behaviour lives in LoginForm.
	import LoginForm from '$lib/components/LoginForm.svelte';
</script>

<svelte:head>
	<title>Login | CoveFlow</title>
</svelte:head>

<LoginForm />
