<script lang="ts">
	// Route page only renders the form. Centering and bg live in user/+layout.svelte,
	// the card frame lives in AuthCard.svelte, and behaviour lives in SignupForm.
	import SignupForm from '$lib/components/SignupForm.svelte';
</script>

<svelte:head>
	<title>Sign Up | CoveFlow</title>
</svelte:head>

<SignupForm />
