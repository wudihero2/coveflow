<script lang="ts">
	import Select from '$lib/components/common/Select.svelte';

	interface WorkspaceOption {
		id: string;
		name: string;
	}

	interface Props {
		workspaces: WorkspaceOption[];
		current: string;
		onSwitch?: (id: string) => void;
	}

	let { workspaces, current, onSwitch }: Props = $props();

	let options = $derived(workspaces.map((ws) => ({ label: ws.name, value: ws.id })));
	let currentName = $derived(workspaces.find((ws) => ws.id === current)?.name ?? current);
</script>

<div class="px-3 py-1.5">
	<p class="text-[11px] font-semibold uppercase tracking-wider text-text-tertiary">Workspace</p>

	{#if workspaces.length > 1}
		<!-- Multiple workspaces: dropdown to switch. -->
		<div class="mt-1">
			<Select
				ariaLabel="Switch workspace"
				{options}
				value={current}
				compact
				onchange={onSwitch}
			/>
		</div>
	{:else}
		<!-- Single workspace: static name. -->
		<p class="mt-0.5 truncate text-sm font-medium text-text" title={currentName}>
			{currentName}
		</p>
	{/if}
</div>
