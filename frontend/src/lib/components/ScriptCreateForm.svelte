<script lang="ts">
	// Third-party icon component from lucide-svelte.
	import { Plus } from '@lucide/svelte';

	import Button from '$lib/components/common/Button.svelte';
	import Select from '$lib/components/common/Select.svelte';
	import TextArea from '$lib/components/common/TextArea.svelte';
	import TextInput from '$lib/components/common/TextInput.svelte';
	import { api, ApiClientError } from '$lib/services/api';
	import { workspace } from '$lib/stores/workspace.svelte';
	import { toastError, toastSuccess } from '$lib/toast';
	import type { ScriptLang } from '$lib/types';

	interface Props {
		oncreated?: () => void;
	}

	// Parent pages can pass oncreated to refresh their script list after save.
	let { oncreated }: Props = $props();

	// Local draft state for the create-script form.
	let name = $state('');
	let path = $state('');
	let language = $state<ScriptLang>('python3');
	let summary = $state('');
	let content = $state('print("hello from CoveFlow")\n');
	let saving = $state(false);

	const languageOptions = [{ label: 'python3', value: 'python3' }];

	// Persist a new script version, then notify the parent to reload data.
	async function createScript() {
		const wsId = workspace.id;
		saving = true;

		try {
			const response = await api.forWorkspace(wsId).createScript({
				path,
				name: name.trim(),
				language,
				content,
				summary: summary || undefined
			});
			// Stale guard: if user switched workspace during the request, the
			// script was created in the old workspace. Don't toast or reload
			// in the new workspace context — it would be misleading.
			if (workspace.id !== wsId) return;
			toastSuccess(`Saved ${response.hash.slice(0, 12)}`);
			oncreated?.();
		} catch (e) {
			if (workspace.id !== wsId) return;
			if (e instanceof ApiClientError) {
				toastError(`${e.status}: ${e.body || e.message}`);
			} else {
				toastError(e instanceof Error ? e.message : 'Request failed');
			}
		} finally {
			saving = false;
		}
	}
</script>

<form
	class="space-y-4"
	onsubmit={(event) => {
		event.preventDefault();
		void createScript();
	}}
>
	<div class="flex items-center gap-2">
		<Plus size={17} class="text-accent" />
		<h2 class="text-lg font-semibold text-text">New script</h2>
	</div>

	<TextInput id="name" label="Name" bind:value={name} placeholder="Extract users" required />

	<TextInput
		id="path"
		label="Path"
		bind:value={path}
		placeholder="users/you@example.com/hello"
		mono
		required
	/>

	<Select id="language" label="Language" options={languageOptions} bind:value={language} />

	<TextInput id="summary" label="Summary" bind:value={summary} placeholder="Initial version" />

	<TextArea id="content" label="Content" bind:value={content} mono required />

	<Button variant="primary" type="submit" loading={saving} class="w-full">Save script</Button>
</form>
