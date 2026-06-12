<script lang="ts">
	import TextInput from '$lib/components/common/TextInput.svelte';
	import { LABELS } from './labels';

	interface Props {
		value?: string;
		disabled: boolean;
		readonly: boolean;
		onValueChange: (value: string) => void;
	}

	let { value = $bindable(''), disabled, readonly, onValueChange }: Props = $props();

	function handleInput(event: Event) {
		value = (event.currentTarget as HTMLInputElement).value;
		onValueChange(value);
	}
</script>

<div class="border-b border-border bg-surface px-3 py-2">
	<TextInput
		label={LABELS.requirements.label}
		bind:value
		oninput={handleInput}
		placeholder={LABELS.requirements.placeholder}
		disabled={disabled || readonly}
		mono
	/>
</div>
