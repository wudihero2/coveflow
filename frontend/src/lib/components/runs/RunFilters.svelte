<script lang="ts">
	import Select from '$lib/components/common/Select.svelte';
	import TextInput from '$lib/components/common/TextInput.svelte';
	import TimeRangePicker from '$lib/components/common/TimeRangePicker.svelte';

	interface TimeRange {
		after?: number;
		before?: number;
	}

	interface Props {
		status: string;
		kind: string;
		createdBy: string;
		scriptPath: string;
		timeRange: TimeRange;
	}

	let {
		status = $bindable(''),
		kind = $bindable(''),
		createdBy = $bindable(''),
		scriptPath = $bindable(''),
		timeRange = $bindable({})
	}: Props = $props();

	const statusOptions = [
		{ label: 'All statuses', value: '' },
		{ label: 'Queued', value: 'queued' },
		{ label: 'Running', value: 'running' },
		{ label: 'Success', value: 'success' },
		{ label: 'Failed', value: 'failure' },
		{ label: 'Cancelled', value: 'cancelled' }
	];

	const kindOptions = [
		{ label: 'All kinds', value: '' },
		{ label: 'Script', value: 'script' },
		{ label: 'Flow', value: 'flow' },
		{ label: 'Preview', value: 'preview' },
		{ label: 'Flow preview', value: 'flow_preview' },
		{ label: 'Maintenance', value: 'maintenance' }
	];
</script>

<div class="flex flex-wrap items-end gap-3">
	<div class="w-48">
		<span class="block text-sm font-medium text-text-secondary">Time</span>
		<div class="mt-2">
			<TimeRangePicker bind:value={timeRange} />
		</div>
	</div>
	<div class="w-40">
		<Select label="Status" options={statusOptions} bind:value={status} />
	</div>
	<div class="w-40">
		<Select label="Kind" options={kindOptions} bind:value={kind} />
	</div>
	<div class="w-56">
		<TextInput label="Created by" bind:value={createdBy} placeholder="user@example.com" />
	</div>
	<div class="w-64">
		<TextInput label="Path" bind:value={scriptPath} placeholder="script or flow path" mono />
	</div>
</div>
