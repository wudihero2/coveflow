<script lang="ts" module>
	type TreeFile<T> = { type: 'file'; name: string; item: T };
	type TreeFolder<T> = { type: 'folder'; name: string; path: string; children: TreeNode<T>[] };
	type TreeNode<T> = TreeFile<T> | TreeFolder<T>;

	function buildTree<T extends { path: string }>(
		items: T[],
		emptyFolders: string[] = []
	): TreeNode<T>[] {
		const root: TreeFolder<T> = { type: 'folder', name: '', path: '', children: [] };
		// Ensure a folder chain exists (no file at the leaf) — used to always show
		// the workspace/personal/team roots even when they contain no files yet.
		function ensureFolder(path: string): void {
			const segs = path.split('/');
			let cur = root;
			for (let i = 0; i < segs.length; i++) {
				const fpath = segs.slice(0, i + 1).join('/');
				let next = cur.children.find(
					(c): c is TreeFolder<T> => c.type === 'folder' && c.name === segs[i]
				);
				if (!next) {
					next = { type: 'folder', name: segs[i], path: fpath, children: [] };
					cur.children.push(next);
				}
				cur = next;
			}
		}
		for (const f of emptyFolders) if (f) ensureFolder(f);
		for (const item of items) {
			const segs = item.path.split('/');
			let cur = root;
			for (let i = 0; i < segs.length - 1; i++) {
				const fpath = segs.slice(0, i + 1).join('/');
				let next = cur.children.find(
					(c): c is TreeFolder<T> => c.type === 'folder' && c.name === segs[i]
				);
				if (!next) {
					next = { type: 'folder', name: segs[i], path: fpath, children: [] };
					cur.children.push(next);
				}
				cur = next;
			}
			cur.children.push({ type: 'file', name: segs[segs.length - 1], item });
		}
		sortFolder(root);
		return root.children;
	}
	function sortFolder<T>(folder: TreeFolder<T>): void {
		folder.children.sort((a, b) => {
			if (a.type !== b.type) return a.type === 'folder' ? -1 : 1;
			return a.name.localeCompare(b.name);
		});
		for (const c of folder.children) if (c.type === 'folder') sortFolder(c);
	}
</script>

<script lang="ts" generics="T extends { path: string }">
	import {
		ChevronRight,
		ChevronDown,
		Code,
		FolderClosed,
		FolderOpen,
		Plus,
		Trash2,
		type Icon as IconType
	} from '@lucide/svelte';

	interface Props {
		items: T[];
		mode: 'explorer' | 'picker';
		selectedPath?: string;
		/** Display label for an item (default: the path's leaf). */
		label?: (item: T) => string;
		/** Singular noun used in UI strings, e.g. "script" / "flow". */
		noun?: string;
		/** Folders always shown (even when empty), e.g. the workspace/team roots. */
		rootFolders?: string[];
		/** Leaf icon for all items (used when `iconFor` is absent). */
		icon?: typeof IconType;
		/** Per-item leaf icon (overrides `icon`) — e.g. script vs flow. */
		iconFor?: (item: T) => typeof IconType;
		/** explorer: navigate to the file; picker: add it as a node. */
		onSelect?: (item: T) => void;
		onNewInFolder?: (folder: string) => void;
		onMove?: (item: T, newPath: string) => void;
		onDelete?: (item: T) => void;
		/** Right-click on a folder row (explorer): for a context menu. */
		onFolderContextMenu?: (folderPath: string, e: MouseEvent) => void;
	}
	let {
		items,
		mode,
		selectedPath,
		label = (item: T) => item.path.split('/').pop() ?? item.path,
		noun = 'item',
		rootFolders = [],
		icon: LeafIcon = Code,
		iconFor,
		onSelect,
		onNewInFolder,
		onMove,
		onDelete,
		onFolderContextMenu
	}: Props = $props();

	let filter = $state('');
	let expanded = $state<Record<string, boolean>>({});
	const searching = $derived(filter.trim() !== '');

	const tree = $derived.by(() => {
		const q = filter.trim().toLowerCase();
		const list = q
			? items.filter(
					(s) => label(s).toLowerCase().includes(q) || s.path.toLowerCase().includes(q)
				)
			: items;
		// Only inject the always-on roots when not searching.
		return buildTree(list, q ? [] : rootFolders);
	});
	// Search shows everything expanded; otherwise honor per-folder state.
	function isOpen(path: string): boolean {
		return searching || expanded[path] === true;
	}

	// Drag state (explorer move).
	let dragItem = $state<T | null>(null);
	let dropTarget = $state<string | null>(null);
	// Inline rename state (keyed by file path).
	let renaming = $state<string | null>(null);
	let renameValue = $state('');

	function startRename(item: T): void {
		renaming = item.path;
		renameValue = label(item);
	}
	function commitRename(item: T): void {
		// Guard: Escape (sets renaming=null) and a duplicate blur-after-Enter must
		// not commit. Only the still-active rename of this exact item proceeds.
		if (renaming !== item.path) return;
		renaming = null;
		const name = renameValue.trim();
		if (!name || name === label(item) || name.includes('/')) return;
		const i = item.path.lastIndexOf('/');
		const parent = i === -1 ? '' : item.path.slice(0, i);
		onMove?.(item, parent ? `${parent}/${name}` : name);
	}
	function dropOnFolder(folderPath: string): void {
		const item = dragItem;
		dragItem = null;
		dropTarget = null;
		if (!item) return;
		const leaf = item.path.split('/').pop() ?? item.path;
		const newPath = folderPath ? `${folderPath}/${leaf}` : leaf;
		if (newPath !== item.path) onMove?.(item, newPath);
	}
</script>

<div class="flex min-h-0 flex-1 flex-col gap-1.5">
	{#if items.length > 0}
		<input
			class="w-full rounded-md border border-border bg-surface-raised px-2 py-1 text-xs text-text"
			placeholder="Search {noun}s…"
			bind:value={filter}
		/>
	{/if}
	<div class="flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto">
		{#if tree.length === 0}
			<p class="px-1 text-xs text-text-tertiary">
				{searching ? `No ${noun}s match.` : `No ${noun}s in this workspace.`}
			</p>
		{:else}
			{@render nodes(tree, 0)}
		{/if}
	</div>
</div>

{#snippet nodes(list: TreeNode<T>[], depth: number)}
	{#each list as n (n.type === 'folder' ? `d:${n.path}` : `f:${n.item.path}`)}
		{#if n.type === 'folder'}
			<!-- Container is the drop target; the toggle + "new" are non-nested siblings. -->
			<div
				role="treeitem"
				tabindex="-1"
				aria-expanded={isOpen(n.path)}
				aria-selected={false}
				class="group relative flex items-center rounded hover:bg-surface-alt"
				class:bg-accent-subtle={dropTarget === n.path}
				ondragover={mode === 'explorer'
					? (e) => {
							e.preventDefault();
							dropTarget = n.path;
						}
					: undefined}
				ondragleave={() => dropTarget === n.path && (dropTarget = null)}
				ondrop={mode === 'explorer' ? () => dropOnFolder(n.path) : undefined}
				oncontextmenu={mode === 'explorer' && onFolderContextMenu
					? (e) => {
							e.preventDefault();
							onFolderContextMenu(n.path, e);
						}
					: undefined}
			>
				<button
					type="button"
					class="flex w-full items-center gap-1 px-1 py-1 text-left text-xs text-text-secondary"
					style="padding-left: {depth * 12 + 4}px"
					onclick={() => {
						if (!searching) expanded[n.path] = !expanded[n.path];
					}}
				>
					{#if isOpen(n.path)}
						<ChevronDown size={13} class="shrink-0 text-text-tertiary" />
						<FolderOpen size={13} class="shrink-0 text-text-tertiary" />
					{:else}
						<ChevronRight size={13} class="shrink-0 text-text-tertiary" />
						<FolderClosed size={13} class="shrink-0 text-text-tertiary" />
					{/if}
					<span class="truncate">{n.name}</span>
				</button>
				{#if mode === 'explorer' && onNewInFolder}
					<button
						type="button"
						class="absolute top-1/2 right-1 -translate-y-1/2 opacity-0 transition group-hover:opacity-100"
						title="New {noun} in this folder"
						onclick={() => onNewInFolder(n.path)}
					>
						<Plus size={13} class="text-text-tertiary hover:text-accent" />
					</button>
				{/if}
			</div>
			{#if isOpen(n.path)}
				{@render nodes(n.children, depth + 1)}
			{/if}
		{:else}
			{@const Icon = iconFor?.(n.item) ?? LeafIcon}
			<div
				role="treeitem"
				tabindex="-1"
				aria-selected={selectedPath === n.item.path}
				class="group flex items-center gap-1 rounded px-1 py-1 text-xs hover:bg-surface-alt"
				class:bg-accent-subtle={mode === 'explorer' && selectedPath === n.item.path}
				style="padding-left: {depth * 12 + 4}px"
				draggable={mode === 'explorer' && renaming !== n.item.path}
				ondragstart={() => (dragItem = n.item)}
				ondragend={() => {
					dragItem = null;
					dropTarget = null;
				}}
			>
				<Icon size={13} class="shrink-0 text-accent" />
				{#if renaming === n.item.path}
					<!-- svelte-ignore a11y_autofocus -->
					<input
						class="min-w-0 flex-1 rounded border border-border bg-surface-raised px-1 text-xs"
						bind:value={renameValue}
						autofocus
						onblur={() => commitRename(n.item)}
						onkeydown={(e) => {
							if (e.key === 'Enter') commitRename(n.item);
							else if (e.key === 'Escape') renaming = null;
						}}
					/>
				{:else}
					<button
						type="button"
						class="min-w-0 flex-1 truncate text-left text-text"
						title={n.item.path}
						onclick={() => onSelect?.(n.item)}
						ondblclick={mode === 'explorer' ? () => startRename(n.item) : undefined}
					>
						{n.name}
					</button>
					{#if mode === 'explorer' && onDelete}
						<button
							type="button"
							class="shrink-0 opacity-0 transition group-hover:opacity-100"
							title="Delete {noun}"
							onclick={() => onDelete(n.item)}
						>
							<Trash2 size={12} class="text-text-tertiary hover:text-error" />
						</button>
					{:else if mode === 'picker'}
						<Plus
							size={12}
							class="ml-auto shrink-0 text-text-tertiary opacity-0 group-hover:opacity-100"
						/>
					{/if}
				{/if}
			</div>
		{/if}
	{/each}
{/snippet}
