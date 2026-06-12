<script lang="ts">
	import { FileCode, Play, Users } from '@lucide/svelte';

	import Alert from '$lib/components/common/Alert.svelte';
	import Card from '$lib/components/common/Card.svelte';
	import PageFrame from '$lib/components/common/PageFrame.svelte';
	import { useWorkspaceLoader } from '$lib/services/workspace-loader.svelte';
	import { workspace } from '$lib/stores/workspace.svelte';
	import type { RunListItem, ScriptListItem, UserInfo } from '$lib/types';

	const meLoader = useWorkspaceLoader<UserInfo>((ws) => ws.getMe());
	const scriptsLoader = useWorkspaceLoader<ScriptListItem[]>((ws) => ws.listScripts());
	const runsLoader = useWorkspaceLoader<RunListItem[]>((ws) =>
		ws.listRuns({ created_after_ms: Date.now() - 24 * 60 * 60 * 1000, limit: 200 })
	);

	const runsLast24h = $derived(runsLoader.data ?? []);
	const scriptCount = $derived(scriptsLoader.data?.length ?? 0);

</script>

<svelte:head>
	<title>Home | CoveFlow</title>
</svelte:head>

<PageFrame title="Home">
	{#if meLoader.error || scriptsLoader.error || runsLoader.error}
		<Alert variant="error">
			{meLoader.error || scriptsLoader.error || runsLoader.error}
		</Alert>
	{/if}

	<div class="mt-6 grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
		<!-- Workspace Info -->
		<Card>
			<div class="flex items-start gap-3">
				<div
					class="flex h-10 w-10 shrink-0 items-center justify-center rounded-lg bg-accent-subtle"
				>
					<Users size={20} class="text-accent" />
				</div>
				<div>
					<p class="text-xs font-medium uppercase tracking-wider text-text-tertiary">
						Workspace
					</p>
					<p class="mt-1 text-lg font-semibold text-text">{workspace.id}</p>
					{#if meLoader.data}
						<p class="mt-0.5 text-sm text-text-secondary">
							{meLoader.data.email} &middot;
							<span class="capitalize">{meLoader.data.role}</span>
						</p>
					{/if}
				</div>
			</div>
		</Card>

		<!-- Scripts -->
		<a href="/scripts" class="block transition hover:ring-2 hover:ring-accent/20 rounded-lg">
			<Card>
				<div class="flex items-start gap-3">
					<div
						class="flex h-10 w-10 shrink-0 items-center justify-center rounded-lg bg-accent-subtle"
					>
						<FileCode size={20} class="text-accent" />
					</div>
					<div>
						<p class="text-xs font-medium uppercase tracking-wider text-text-tertiary">
							Scripts
						</p>
						<p class="mt-1 text-lg font-semibold text-text">
							{scriptsLoader.loading ? '…' : scriptCount}
						</p>
						<p class="mt-0.5 text-sm text-text-secondary">Total scripts</p>
					</div>
				</div>
			</Card>
		</a>

		<!-- Runs Last 24h -->
		<a href="/runs" class="block transition hover:ring-2 hover:ring-accent/20 rounded-lg">
			<Card>
				<div class="flex items-start gap-3">
					<div
						class="flex h-10 w-10 shrink-0 items-center justify-center rounded-lg bg-accent-subtle"
					>
						<Play size={20} class="text-accent" />
					</div>
					<div>
						<p class="text-xs font-medium uppercase tracking-wider text-text-tertiary">
							Runs
						</p>
						<p class="mt-1 text-lg font-semibold text-text">
							{runsLoader.loading ? '…' : runsLast24h.length}
						</p>
						<p class="mt-0.5 text-sm text-text-secondary">Last 24 hours</p>
					</div>
				</div>
			</Card>
		</a>
	</div>
</PageFrame>
