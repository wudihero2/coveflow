<script lang="ts">
	interface Tab {
		key: string;
		label: string;
	}

	interface Props {
		tabs: Tab[];
		active: string;
	}

	let { tabs, active = $bindable('') }: Props = $props();
</script>

<div class="flex gap-1 border-b border-border" role="tablist">
	{#each tabs as tab (tab.key)}
		<button
			role="tab"
			aria-selected={active === tab.key}
			class="px-3 py-2 text-sm font-medium transition {active === tab.key
				? 'border-b-2 border-accent text-accent'
				: 'text-text-tertiary hover:text-text-secondary'}"
			onclick={() => (active = tab.key)}
		>
			{tab.label}
		</button>
	{/each}
</div>
