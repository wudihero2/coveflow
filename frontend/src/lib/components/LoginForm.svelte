<script lang="ts">
	// SvelteKit client-side navigation helper.
	import { goto } from '$app/navigation';
	import { page } from '$app/state';

	import Alert from '$lib/components/common/Alert.svelte';
	import AuthCard from '$lib/components/auth/AuthCard.svelte';
	import Button from '$lib/components/common/Button.svelte';
	import PasswordInput from '$lib/components/common/PasswordInput.svelte';
	import TextInput from '$lib/components/common/TextInput.svelte';
	// Central auth store handles login, token refresh scheduling, and workspaces.
	import { auth } from '$lib/stores/auth.svelte';
	import { workspace } from '$lib/stores/workspace.svelte';

	// Local form state. These fields are reset only by the browser/page lifecycle.
	let email = $state('');
	let password = $state('');
	let error = $state('');
	let loading = $state(false);
	let nextPath = $derived(page.url.searchParams.get('next'));
	let requestedWorkspaceId = $derived(page.url.searchParams.get('workspace'));

	function postLoginPath(): string {
		if (!nextPath || !nextPath.startsWith('/') || nextPath.startsWith('//')) {
			return '/scripts';
		}
		return nextPath;
	}

	function restoreRequestedWorkspace(): boolean {
		if (!requestedWorkspaceId) return true;
		if (!workspace.list.some((w) => w.id === requestedWorkspaceId)) return false;
		workspace.switch(requestedWorkspaceId);
		return true;
	}

	// Login first, then move into the authenticated route group.
	async function submit() {
		error = '';
		loading = true;

		try {
			await auth.login(email, password);
			await goto(restoreRequestedWorkspace() ? postLoginPath() : '/scripts');
		} catch (e) {
			error = e instanceof Error ? e.message : 'Login failed';
		} finally {
			loading = false;
		}
	}
</script>

<AuthCard title="Login">
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
			autocomplete="current-password"
			required
		/>

		{#if error}
			<Alert variant="error">{error}</Alert>
		{/if}

		<div class="pt-2">
			<Button variant="primary" size="lg" type="submit" {loading} class="w-full">Log in</Button>
		</div>

		<p class="text-center text-sm text-text-tertiary">
			No account yet?
			<a class="font-medium text-accent hover:text-accent-hover" href="/user/signup">Sign up</a>
		</p>
	</form>
</AuthCard>
