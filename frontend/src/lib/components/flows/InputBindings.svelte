<script lang="ts">
	import { untrack } from 'svelte';
	import { Plus, X, Wand2 } from '@lucide/svelte';
	import type { InputBinding } from '$lib/types';

	interface Row {
		id: number;
		name: string;
		kind: 'static' | 'expr';
		value: string;
	}

	// Stable per-row id so #each keys survive reorder/delete (avoids the
	// index-as-key bind footgun).
	let nextId = 0;

	interface Props {
		/** Current bindings: main() param name -> binding. */
		inputs: Record<string, InputBinding> | undefined;
		/** Upstream step ids that may be referenced as steps.<id>.result. */
		steps: string[];
		onCommit: (inputs: Record<string, InputBinding> | undefined) => void;
		/** Resolve the script's required main() params (for the detect button). */
		onDetect?: () => Promise<string[]>;
	}
	let { inputs, steps, onCommit, onDetect }: Props = $props();

	// main() params the worker injects itself (auto-supplied at run time), so they
	// are never flow inputs — keep them out of "detect from script".
	const RESERVED_PARAMS = new Set(['ctx']);

	let detecting = $state(false);
	async function detect(): Promise<void> {
		if (!onDetect) return;
		detecting = true;
		try {
			const names = await onDetect();
			const existing = new Set(rows.map((r) => r.name.trim()));
			const additions = names
				.filter((n) => !existing.has(n) && !RESERVED_PARAMS.has(n))
				.map((n) => ({ id: nextId++, name: n, kind: 'expr' as const, value: '' }));
			if (additions.length) {
				rows = [...rows, ...additions];
				emit();
			}
		} finally {
			detecting = false;
		}
	}

	function toRows(map: Record<string, InputBinding> | undefined): Row[] {
		return Object.entries(map ?? {}).map(([name, b]) =>
			b.kind === 'expr'
				? { id: nextId++, name, kind: 'expr', value: b.expr }
				: {
						id: nextId++,
						name,
						kind: 'static',
						value: typeof b.value === 'string' ? b.value : JSON.stringify(b.value)
					}
		);
	}

	// Keyed per selection in the parent, so `inputs` is only the initial seed.
	let rows = $state<Row[]>(untrack(() => toRows(inputs)));

	function parseStatic(v: string): unknown {
		const t = v.trim();
		if (t === '') return '';
		try {
			return JSON.parse(t);
		} catch {
			return v;
		}
	}
	function emit(): void {
		const obj: Record<string, InputBinding> = {};
		for (const r of rows) {
			const name = r.name.trim();
			if (!name) continue;
			obj[name] =
				r.kind === 'expr'
					? { kind: 'expr', expr: r.value }
					: { kind: 'static', value: parseStatic(r.value) };
		}
		onCommit(Object.keys(obj).length ? obj : undefined);
	}
	function addRow(): void {
		rows = [...rows, { id: nextId++, name: '', kind: 'static', value: '' }];
	}
	function removeRow(id: number): void {
		rows = rows.filter((r) => r.id !== id);
		emit();
	}
	// Append a reference token into an expression row, so users don't have to
	// remember/spell ids or the run.* field names. They can then drill in (`.field`).
	function insertToken(row: Row, token: string): void {
		if (!token) return;
		row.value = row.value ? `${row.value}${token}` : token;
		emit();
	}
	function insertStepRef(row: Row, id: string): void {
		if (id) insertToken(row, `steps.${id}.result`);
	}

	// Airflow-style execution context, available in every expression at run time.
	// A fixed list (unlike steps, which depend on the DAG) — see RunContext.
	const RUN_VARS = [
		'run.logical_date',
		'run.ds',
		'run.ts',
		'run.data_interval_start',
		'run.data_interval_end',
		'run.timezone',
		'run.is_scheduled',
		'run.schedule_name',
		'run.run_id',
		'run.flow_path',
		'run.triggered_at'
	];

	const SELECT = 'rounded-md border border-border bg-surface-raised px-1.5 py-1 text-xs text-text';
	const INPUT =
		'min-w-0 flex-1 rounded-md border border-border bg-surface-raised px-2 py-1 text-xs text-text';
</script>

<div class="flex flex-col gap-1.5">
	<span class="text-xs text-text-tertiary">inputs → main() parameters</span>
	{#each rows as row (row.id)}
		<div class="flex flex-col gap-1 rounded-md border border-border bg-surface-alt p-1.5">
			<div class="flex items-center gap-1">
				<input
					class="{INPUT} font-mono"
					placeholder="param name"
					bind:value={row.name}
					onchange={emit}
				/>
				<select class={SELECT} bind:value={row.kind} onchange={emit}>
					<option value="static">value</option>
					<option value="expr">expression</option>
				</select>
				<button
					type="button"
					class="text-text-tertiary hover:text-error"
					title="Remove input"
					onclick={() => removeRow(row.id)}
				>
					<X size={13} />
				</button>
			</div>
			<div class="flex items-center gap-1">
				<input
					class="{INPUT} font-mono"
					placeholder={row.kind === 'expr'
						? 'flow.input.x  /  steps.fetch.result.id'
						: 'e.g. 5, true, "hello"'}
					bind:value={row.value}
					onchange={emit}
				/>
				{#if row.kind === 'expr'}
					<!-- Insert a reference token; resets to the placeholder after each
					     pick so it can be used repeatedly. -->
					{#if steps.length}
						<select
							class="{SELECT} w-20 shrink-0 truncate"
							title="Insert upstream result"
							value=""
							onchange={(e) => {
								insertStepRef(row, e.currentTarget.value);
								e.currentTarget.value = '';
							}}
						>
							<option value="" disabled selected>steps ↧</option>
							{#each steps as s (s)}
								<option value={s}>steps.{s}.result</option>
							{/each}
						</select>
					{/if}
					<select
						class="{SELECT} w-16 shrink-0 truncate"
						title="Insert run context variable"
						value=""
						onchange={(e) => {
							insertToken(row, e.currentTarget.value);
							e.currentTarget.value = '';
						}}
					>
						<option value="" disabled selected>run ↧</option>
						{#each RUN_VARS as v (v)}
							<option value={v}>{v}</option>
						{/each}
					</select>
				{/if}
			</div>
		</div>
	{/each}
	<div class="flex items-center gap-3">
		<button
			type="button"
			class="flex items-center gap-1 text-[11px] text-accent hover:underline"
			onclick={addRow}
		>
			<Plus size={12} /> add input
		</button>
		{#if onDetect}
			<button
				type="button"
				class="flex items-center gap-1 text-[11px] text-accent hover:underline disabled:opacity-50"
				disabled={detecting}
				onclick={detect}
			>
				<Wand2 size={12} /> {detecting ? 'detecting…' : 'detect from script'}
			</button>
		{/if}
	</div>
	<span class="text-[11px] text-text-tertiary">
		Expressions use <code>flow.input.*</code>, <code>steps.&lt;id&gt;.result.*</code>{#if steps.length}
			(upstream: {steps.join(', ')}){/if} and <code>run.*</code> (logical date, data interval, schedule
		meta). The script's <code>main()</code> receives these as keyword arguments.
	</span>
</div>
