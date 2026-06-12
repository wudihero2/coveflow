<script lang="ts">
	// SvelteKit client-side navigation helper.
	import { goto } from '$app/navigation';

	import Alert from '$lib/components/common/Alert.svelte';
	import AuthCard from '$lib/components/auth/AuthCard.svelte';
	import Button from '$lib/components/common/Button.svelte';
	import PasswordInput from '$lib/components/common/PasswordInput.svelte';
	import TextInput from '$lib/components/common/TextInput.svelte';
	// Central auth store handles signup, token refresh scheduling, and workspaces.
	import { auth } from '$lib/stores/auth.svelte';

	// Local form state. Signup success promotes the user into the app shell.
	let email = $state('');
	let password = $state('');
	let error = $state('');
	let loading = $state(false);

	// Create the account, hydrate auth/workspace state, then enter /scripts.
	async function submit() {
		error = '';
		loading = true;

		try {
			await auth.signup(email, password);
			await goto('/scripts');
		} catch (e) {
			error = e instanceof Error ? e.message : 'Signup failed';
		} finally {
			loading = false;
		}
	}
</script>

<AuthCard title="Sign up">
	<form
		class="space-y-4"
		onsubmit={(event) => {
			event.preventDefault();
			void submit();
		}}
	>
		<TextInput
			id="email"
			label="Email"
			type="email"
			bind:value={email}
			autocomplete="email"
			required
		/>

		<PasswordInput
			id="password"
			label="Password"
			bind:value={password}
			autocomplete="new-password"
			minlength={8}
			required
		/>

		{#if error}
			<Alert variant="error">{error}</Alert>
		{/if}

		<div class="pt-2">
			<Button variant="primary" size="lg" type="submit" {loading} class="w-full">
				Create account
			</Button>
		</div>

		<p class="text-center text-sm text-text-tertiary">
			Already registered?
			<a class="font-medium text-accent hover:text-accent-hover" href="/user/login">Log in</a>
		</p>
	</form>
</AuthCard>
