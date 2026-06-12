<script lang="ts">
	import Alert from '$lib/components/common/Alert.svelte';
	import TextArea from '$lib/components/common/TextArea.svelte';
	import { LABELS } from './labels';

	interface Props {
		content?: string;
		disabled: boolean;
		readonly: boolean;
	}

	let { content = $bindable(''), disabled, readonly }: Props = $props();
</script>

<div class="flex-1 space-y-3 p-3">
	<Alert variant="info">{LABELS.fallback.notice}</Alert>
	<TextArea
		label={LABELS.fallback.label}
		bind:value={content}
		mono
		{disabled}
		{readonly}
		rows={16}
	/>
</div>
