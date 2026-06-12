<script lang="ts">
	import { Code, Workflow, FilePlus, Plus, Shield, type Icon as IconType } from '@lucide/svelte';
	import { goto, afterNavigate } from '$app/navigation';
	import { page } from '$app/state';
	import { getContext } from 'svelte';

	import FileTree from '$lib/components/common/FileTree.svelte';
	import { api, ApiClientError } from '$lib/services/api';
	import { confirmDialog } from '$lib/stores/confirm.svelte';
	import { encodePath } from '$lib/services/url';
	import { workspace } from '$lib/stores/workspace.svelte';
	import { toastError, toastSuccess } from '$lib/toast';

	// One tree holding both scripts and flows. `path` is the tree key; `kind`
	// drives the icon, the editor route, and which move/delete endpoint to call.
	type WorkspaceItem = { path: string; kind: 'script' | 'flow'; script_id?: string };

	let items = $state<WorkspaceItem[]>([]);
	let myEmail = $state('');
	let myTeams = $state<string[]>([]);

	// Always-on roots so empty spaces (e.g. a brand-new team) still show up: the
	// shared Workspace, your personal space, and each team you belong to.
	const rootFolders = $derived(
		['workspace', myEmail ? `users/${myEmail}` : '', ...myTeams.map((t) => `teams/${t}`)].filter(
			Boolean
		)
	);

	// Whichever editor is open (scripts OR flows share this tree).
	const selectedPath = $derived.by(() => {
		const m = page.url.pathname.match(/^\/(?:scripts\/edit|flows\/edit)\/(.+)$/);
		return m ? m[1].split('/').map(decodeURIComponent).join('/') : undefined;
	});

	// Workspace role (for the admin-only "folder permissions" action).
	const roleCtx = getContext<() => { role: string | null }>('auth:role');
	const isAdmin = $derived(roleCtx?.().role === 'admin');

	async function reload(): Promise<void> {
		const ws = workspace.id;
		try {
			const [scripts, flows, me] = await Promise.all([
				api.forWorkspace(ws).listScripts(),
				api.forWorkspace(ws).listFlows(),
				api.forWorkspace(ws).getMe()
			]);
			items = [
				...scripts.map((s) => ({ path: s.path, kind: 'script' as const, script_id: s.script_id })),
				...flows.map((f) => ({ path: f.path, kind: 'flow' as const }))
			];
			myEmail = me.email;
			myTeams = me.teams;
		} catch {
			/* tree is best-effort */
		}
	}
	let lastWs: string | null = null;
	$effect(() => {
		const ws = workspace.id;
		if (ws !== lastWs) {
			lastWs = ws;
			void reload();
		}
	});
	afterNavigate(() => void reload());

	function fmt(e: unknown): string {
		return e instanceof ApiClientError ? `${e.status}: ${e.body || e.message}` : String(e);
	}
	function parseRefs(body: string | undefined): { flows: string[]; active: boolean } {
		try {
			const j = JSON.parse(body ?? '{}');
			return { flows: Array.isArray(j.referenced_by) ? j.referenced_by : [], active: !!j.active_runs };
		} catch {
			return { flows: [], active: false };
		}
	}
	function refsLine(flows: string[], active: boolean): string {
		const parts: string[] = [];
		if (flows.length) parts.push(`flows: ${flows.join(', ')}`);
		if (active) parts.push('in-flight runs');
		return parts.length ? `\n\nReferenced by ${parts.join(' and ')}.` : '';
	}

	function iconFor(it: WorkspaceItem): typeof IconType {
		return it.kind === 'script' ? Code : Workflow;
	}

	function onSelect(it: WorkspaceItem): void {
		const route = it.kind === 'script' ? 'scripts' : 'flows';
		void goto(`/${route}/edit/${encodePath(it.path)}`);
	}

	async function onMove(it: WorkspaceItem, newPath: string): Promise<void> {
		const ws = workspace.id;
		try {
			if (it.kind === 'script') await api.forWorkspace(ws).moveScript(it.script_id!, newPath);
			else await api.forWorkspace(ws).moveFlow(it.path, newPath);
		} catch (e) {
			if (e instanceof ApiClientError && e.status === 409) {
				const { flows, active } = parseRefs(e.body);
				const ok = await confirmDialog({
					title: 'Overwrite?',
					message: `"${newPath}" already exists.${refsLine(flows, active)}`,
					confirmLabel: 'Overwrite',
					variant: 'danger'
				});
				if (!ok) return;
				try {
					if (it.kind === 'script')
						await api.forWorkspace(ws).moveScript(it.script_id!, newPath, true);
					else await api.forWorkspace(ws).moveFlow(it.path, newPath, true);
				} catch (e2) {
					toastError(fmt(e2));
					return;
				}
			} else {
				toastError(fmt(e));
				return;
			}
		}
		toastSuccess('Moved');
		const wasOpen = selectedPath === it.path;
		const route = it.kind === 'script' ? 'scripts' : 'flows';
		await reload();
		if (wasOpen) void goto(`/${route}/edit/${encodePath(newPath)}`);
	}

	async function onDelete(it: WorkspaceItem): Promise<void> {
		const ws = workspace.id;
		try {
			if (it.kind === 'script') {
				const refs = await api.forWorkspace(ws).scriptReferences(it.script_id!);
				const warn = refsLine(refs.flows, refs.active_runs);
				const ok = await confirmDialog({
					title: `Delete "${it.path}"?`,
					message: warn ? `${warn.trim()}\n\nDelete anyway?` : 'This cannot be undone.',
					confirmLabel: 'Delete',
					variant: 'danger'
				});
				if (!ok) return;
				await api.forWorkspace(ws).deleteScript(it.script_id!, !!warn);
			} else {
				const ok = await confirmDialog({
					title: `Delete "${it.path}"?`,
					message: 'This removes all revisions. In-flight runs keep their own snapshot.',
					confirmLabel: 'Delete',
					variant: 'danger'
				});
				if (!ok) return;
				await api.forWorkspace(ws).deleteFlow(it.path);
			}
		} catch (e) {
			if (e instanceof ApiClientError && e.status === 409) {
				const { flows, active } = parseRefs(e.body);
				const ok = await confirmDialog({
					title: `Delete "${it.path}"?`,
					message: `${refsLine(flows, active).trim()}\n\nDelete anyway?`,
					confirmLabel: 'Delete',
					variant: 'danger'
				});
				if (!ok) return;
				try {
					await api.forWorkspace(ws).deleteScript(it.script_id!, true);
				} catch (e2) {
					toastError(fmt(e2));
					return;
				}
			} else {
				toastError(fmt(e));
				return;
			}
		}
		toastSuccess('Deleted');
		const wasOpen = selectedPath === it.path;
		const route = it.kind === 'script' ? 'scripts' : 'flows';
		await reload();
		if (wasOpen) void goto(`/${route}`);
	}

	// Folder right-click context menu.
	let menu = $state<{ x: number; y: number; folder: string } | null>(null);
	function onFolderContextMenu(folder: string, e: MouseEvent): void {
		menu = { x: e.clientX, y: e.clientY, folder };
	}
	function closeMenu(): void {
		menu = null;
	}
	function newScriptHere(): void {
		const f = menu?.folder;
		closeMenu();
		void goto(`/scripts/add?folder=${encodeURIComponent(f ?? '')}`);
	}
	function newFlowHere(): void {
		const f = menu?.folder;
		closeMenu();
		void goto(`/flows/edit?folder=${encodeURIComponent(f ?? '')}`);
	}
	// A `teams/<team>` root → offer team-member management (where access is set).
	function teamOfFolder(folder: string): string {
		const segs = folder.split('/');
		return segs[0] === 'teams' && segs.length === 2 ? (segs[1] ?? '') : '';
	}
	function manageTeam(): void {
		const team = teamOfFolder(menu?.folder ?? '');
		closeMenu();
		if (team) void goto(`/admin/teams/${encodeURIComponent(team)}`);
	}
	function manageWorkspace(): void {
		closeMenu();
		void goto('/admin/members');
	}
</script>

<svelte:window onclick={closeMenu} />

<div class="flex min-h-0 flex-1 flex-col gap-2">
	<div class="flex items-center justify-between">
		<h2 class="text-xs font-semibold tracking-wide text-text-tertiary uppercase">Files</h2>
		<div class="flex gap-1">
			<button
				type="button"
				class="inline-flex items-center gap-1 rounded border border-border px-1.5 py-1 text-[11px] font-medium text-text-secondary hover:bg-surface-alt"
				title="New script (in Workspace)"
				onclick={() => goto('/scripts/add?folder=workspace')}
			>
				<Plus size={11} /><Code size={12} class="text-accent" /> Script
			</button>
			<button
				type="button"
				class="inline-flex items-center gap-1 rounded border border-border px-1.5 py-1 text-[11px] font-medium text-text-secondary hover:bg-surface-alt"
				title="New flow (in Workspace)"
				onclick={() => goto('/flows/edit?folder=workspace')}
			>
				<Plus size={11} /><Workflow size={12} class="text-accent" /> Flow
			</button>
		</div>
	</div>

	<FileTree
		{items}
		mode="explorer"
		noun="file"
		{rootFolders}
		{selectedPath}
		{iconFor}
		{onSelect}
		{onMove}
		{onDelete}
		{onFolderContextMenu}
	/>
</div>

{#if menu}
	<!-- Floating context menu; closed by the window click handler above. -->
	<div
		class="fixed z-50 min-w-44 rounded-md border border-border bg-surface-raised py-1 text-xs shadow-lg"
		style="left: {menu.x}px; top: {menu.y}px"
		role="menu"
		tabindex="-1"
		onclick={(e) => e.stopPropagation()}
		onkeydown={(e) => e.key === 'Escape' && closeMenu()}
	>
		<button
			type="button"
			class="flex w-full items-center gap-2 px-3 py-1.5 text-left text-text hover:bg-surface-alt"
			onclick={newScriptHere}
		>
			<FilePlus size={13} class="text-text-tertiary" /> New script here
		</button>
		<button
			type="button"
			class="flex w-full items-center gap-2 px-3 py-1.5 text-left text-text hover:bg-surface-alt"
			onclick={newFlowHere}
		>
			<FilePlus size={13} class="text-text-tertiary" /> New flow here
		</button>
		{#if isAdmin && teamOfFolder(menu.folder)}
			<div class="my-1 border-t border-border"></div>
			<button
				type="button"
				class="flex w-full items-center gap-2 px-3 py-1.5 text-left text-text hover:bg-surface-alt"
				onclick={manageTeam}
			>
				<Shield size={13} class="text-text-tertiary" /> Manage team members…
			</button>
		{:else if isAdmin && menu.folder === 'workspace'}
			<div class="my-1 border-t border-border"></div>
			<button
				type="button"
				class="flex w-full items-center gap-2 px-3 py-1.5 text-left text-text hover:bg-surface-alt"
				onclick={manageWorkspace}
			>
				<Shield size={13} class="text-text-tertiary" /> Manage members (roles)…
			</button>
		{/if}
	</div>
{/if}
