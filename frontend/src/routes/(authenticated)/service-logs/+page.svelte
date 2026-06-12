<script lang="ts">
	import { goto } from '$app/navigation';
	import { getContext } from 'svelte';
	import { SvelteMap, SvelteSet } from 'svelte/reactivity';

	import Select from '$lib/components/common/Select.svelte';
	import TimeRangePicker from '$lib/components/common/TimeRangePicker.svelte';
	import ServiceLogViewer from '$lib/components/runs/ServiceLogViewer.svelte';
	import { workspace } from '$lib/stores/workspace.svelte';
	import type { WorkspaceRole } from '$lib/types';

	const getAuthRole = getContext<
		() => { role: WorkspaceRole | null; roleLoaded: boolean; roleError: boolean }
	>('auth:role');

	$effect(() => {
		const { role, roleLoaded, roleError } = getAuthRole();
		if (roleLoaded && !roleError && role !== 'admin') {
			void goto('/scripts', { replaceState: true });
		}
	});

	let selectedService = $state<string>('');
	let selectedInstance = $state<string>('');
	let selectedLevel = $state<string>('1');
	let timeRange = $state<{ after?: number; before?: number }>({
		after: Date.now() - 15 * 60 * 1000
	});

	const knownServices = new SvelteSet<string>();
	const instancesByService = new SvelteMap<string, SvelteSet<string>>();

	function handleChunkReceived(service: string, instanceId: string): void {
		knownServices.add(service);
		if (!instancesByService.has(service)) {
			instancesByService.set(service, new SvelteSet<string>());
		}
		instancesByService.get(service)!.add(instanceId);
	}

	const serviceOptions = $derived([
		{ value: '', label: 'All' },
		...Array.from(knownServices)
			.sort()
			.map((s) => ({ value: s, label: s }))
	]);

	const instanceOptions = $derived.by(() => {
		const base: Array<{ value: string; label: string }> = [{ value: '', label: 'All' }];
		if (selectedService) {
			const instances = instancesByService.get(selectedService);
			if (instances) {
				base.push(
					...Array.from(instances)
						.sort()
						.map((i) => ({ value: i, label: i }))
				);
			}
		} else {
			const all = new SvelteSet<string>();
			for (const set of instancesByService.values()) {
				for (const i of set) all.add(i);
			}
			base.push(
				...Array.from(all)
					.sort()
					.map((i) => ({ value: i, label: i }))
			);
		}
		return base;
	});

	const levelOptions = [
		{ value: '1', label: 'ALL' },
		{ value: '2', label: 'DEBUG' },
		{ value: '3', label: 'INFO' },
		{ value: '4', label: 'WARN' },
		{ value: '5', label: 'ERROR' }
	];
</script>

<svelte:head>
	<title>Service Logs | CoveFlow</title>
</svelte:head>

{#if getAuthRole().role === 'admin'}
	<div class="flex h-svh flex-col max-lg:h-[calc(100svh-48px)]">
		<!-- Header row: title + filters -->
		<div class="flex flex-wrap items-end gap-4 border-b border-border px-5 py-3 sm:px-8">
			<h1 class="mr-4 text-base font-semibold text-text">Service Logs</h1>
			<div class="flex flex-wrap items-end gap-4">
				<div class="w-48">
					<span class="block text-xs font-medium text-text-secondary">Time</span>
					<div class="mt-1">
						<TimeRangePicker bind:value={timeRange} />
					</div>
				</div>
				<div class="w-36">
					<Select
						label="Service"
						options={serviceOptions}
						bind:value={selectedService}
						onchange={() => { selectedInstance = ''; }}
						compact
					/>
				</div>
				<div class="w-44">
					<Select
						label="Instance"
						options={instanceOptions}
						bind:value={selectedInstance}
						compact
					/>
				</div>
				<div class="w-28">
					<Select
						label="Level"
						options={levelOptions}
						bind:value={selectedLevel}
						compact
					/>
				</div>
			</div>
		</div>

		<!-- Log viewer fills all remaining height -->
		<ServiceLogViewer
			workspaceId={workspace.id}
			service={selectedService || undefined}
			instance={selectedInstance || undefined}
			minLevel={Number(selectedLevel)}
			sinceMs={timeRange.after}
			untilMs={timeRange.before}
			onChunkReceived={handleChunkReceived}
			class="min-h-0 flex-1"
		/>
	</div>
{/if}
