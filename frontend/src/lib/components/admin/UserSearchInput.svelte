<script lang="ts">
	import { LoaderCircle } from '@lucide/svelte';
	import { api, ApiClientError } from '$lib/services/api';
	import type { UserSearchItem } from '$lib/types';

	interface Props {
		value: string;
		placeholder?: string;
		exclude?: string[];
		class?: string;
	}

	let {
		value = $bindable(''),
		placeholder = 'Search by email…',
		exclude = [],
		class: className = ''
	}: Props = $props();

	let query = $state('');
	let results = $state<UserSearchItem[]>([]);
	let searching = $state(false);
	let showDropdown = $state(false);
	let highlightIndex = $state(-1);
	let debounceTimer: ReturnType<typeof setTimeout> | undefined;
	let searchId = 0;
	let abortCtrl: AbortController | undefined;

	$effect(() => {
		return () => {
			clearTimeout(debounceTimer);
			abortCtrl?.abort();
		};
	});

	const filtered = $derived(results.filter((r) => !exclude.includes(r.email)));

	function scheduleSearch(q: string): void {
		clearTimeout(debounceTimer);
		if (q.length < 2) {
			results = [];
			showDropdown = false;
			return;
		}
		debounceTimer = setTimeout(() => void doSearch(q), 300);
	}

	async function doSearch(q: string): Promise<void> {
		abortCtrl?.abort();
		abortCtrl = new AbortController();
		const id = ++searchId;
		searching = true;
		showDropdown = true;
		highlightIndex = -1;
		try {
			const res = await api.searchUsers(q, abortCtrl.signal);
			if (id !== searchId) return;
			results = res;
		} catch (e) {
			if (id !== searchId) return;
			if (e instanceof DOMException && e.name === 'AbortError') return;
			if (e instanceof ApiClientError) results = [];
		} finally {
			if (id === searchId) searching = false;
		}
	}

	function selectEmail(email: string): void {
		value = email;
		query = email;
		showDropdown = false;
	}

	function handleInput(e: Event): void {
		const target = e.currentTarget as HTMLInputElement;
		query = target.value;
		value = '';
		scheduleSearch(query);
	}

	function handleKeydown(e: KeyboardEvent): void {
		if (!showDropdown || filtered.length === 0) return;
		if (e.key === 'ArrowDown') {
			e.preventDefault();
			highlightIndex = (highlightIndex + 1) % filtered.length;
		} else if (e.key === 'ArrowUp') {
			e.preventDefault();
			highlightIndex = (highlightIndex - 1 + filtered.length) % filtered.length;
		} else if (e.key === 'Enter' && highlightIndex >= 0) {
			e.preventDefault();
			selectEmail(filtered[highlightIndex].email);
		} else if (e.key === 'Escape') {
			showDropdown = false;
		}
	}

	function handleBlur(): void {
		setTimeout(() => (showDropdown = false), 150);
	}

	const uid = $props.id();
	const listboxId = `${uid}-listbox`;
	const activeId = $derived(
		highlightIndex >= 0 && filtered[highlightIndex] ? `${uid}-opt-${highlightIndex}` : undefined
	);
</script>

<div class="relative {className}">
	<input
		type="text"
		value={query}
		oninput={handleInput}
		onkeydown={handleKeydown}
		onblur={handleBlur}
		onfocus={() => {
			if (query.length >= 2) showDropdown = true;
		}}
		{placeholder}
		autocomplete="off"
		role="combobox"
		aria-expanded={showDropdown}
		aria-controls={listboxId}
		aria-activedescendant={activeId}
		aria-autocomplete="list"
		class="h-10 w-full rounded-md border border-border bg-surface px-3 text-sm text-text outline-none transition placeholder:text-text-tertiary focus:border-accent focus:ring-1 focus:ring-accent"
	/>

	{#if showDropdown}
		<div
			id={listboxId}
			role="listbox"
			class="absolute left-0 right-0 top-full z-10 mt-1 max-h-48 overflow-y-auto rounded-md border border-border bg-surface-raised shadow-lg"
		>
			{#if searching}
				<div class="flex items-center gap-2 px-3 py-2 text-sm text-text-tertiary">
					<LoaderCircle size={14} class="animate-spin" />
					Searching…
				</div>
			{:else if filtered.length === 0}
				<div class="px-3 py-2 text-sm text-text-tertiary">No users found</div>
			{:else}
				{#each filtered as item, i (item.email)}
					<button
						type="button"
						id="{uid}-opt-{i}"
						role="option"
						aria-selected={i === highlightIndex}
						class="block w-full px-3 py-2 text-left text-sm text-text hover:bg-surface-alt {i ===
						highlightIndex
							? 'bg-surface-alt'
							: ''}"
						onmousedown={() => selectEmail(item.email)}
					>
						{item.email}
					</button>
				{/each}
			{/if}
		</div>
	{/if}
</div>
