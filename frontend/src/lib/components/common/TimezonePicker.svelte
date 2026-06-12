<script lang="ts">
	import { Globe } from '@lucide/svelte';

	import Select from '$lib/components/common/Select.svelte';
	import { displayTz, TZ_OPTIONS } from '$lib/stores/timezone.svelte';

	// Display-timezone picker (how times render across the app). Does NOT affect
	// when schedules fire — that's each schedule's own timezone.
	const browserZone = Intl.DateTimeFormat().resolvedOptions().timeZone;
	const options = TZ_OPTIONS.map((t) => ({
		label: t === 'Local' ? `Local (${browserZone})` : t,
		value: t
	}));
</script>

<label class="flex items-center gap-1.5 text-xs text-text-tertiary" title="Display timezone">
	<Globe size={13} class="shrink-0" />
	<Select
		ariaLabel="Display timezone"
		{options}
		value={displayTz.value}
		compact
		onchange={(v) => displayTz.set(v)}
	/>
</label>
