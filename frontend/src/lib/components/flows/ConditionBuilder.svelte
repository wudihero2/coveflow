<script lang="ts">
	import { untrack } from 'svelte';
	import { Plus, X, Check, TriangleAlert } from '@lucide/svelte';
	import { api } from '$lib/services/api';
	import { workspace } from '$lib/stores/workspace.svelte';

	type Op = 'true' | 'false' | '==' | '!=' | '<' | '<=' | '>' | '>=';
	interface Row {
		id: number;
		src: string; // 'flow.input' or an upstream step id
		path: string;
		op: Op;
		value: string;
		// Preserves "this was a quoted string" across round-trips, so e.g.
		// `code == "5"` doesn't collapse to `code == 5` (different JSON type).
		quoted: boolean;
	}

	// Stable per-row id so #each keys survive reorder/delete.
	let nextId = 0;

	interface Props {
		/** Current expression (undefined = no condition). */
		value: string | undefined;
		/** Upstream step ids that may be referenced as steps.<id>.result. */
		steps: string[];
		/** Called when the user commits a change (blur / selection). */
		onCommit: (expr: string | undefined) => void;
	}
	let { value, steps, onCommit }: Props = $props();

	const OPS: { v: Op; label: string }[] = [
		{ v: 'true', label: 'is true' },
		{ v: 'false', label: 'is false' },
		{ v: '==', label: 'equals (==)' },
		{ v: '!=', label: 'not equal (!=)' },
		{ v: '<', label: 'less than (<)' },
		{ v: '<=', label: 'at most (<=)' },
		{ v: '>', label: 'greater than (>)' },
		{ v: '>=', label: 'at least (>=)' }
	];

	function blankRow(): Row {
		return { id: nextId++, src: 'flow.input', path: '', op: '==', value: '', quoted: false };
	}

	// ---- expression <-> rows --------------------------------------------------
	// Render a row value to an expression literal. `quoted` forces a string literal
	// (preserving round-trips like `== "5"`); otherwise bare true/false/null/number
	// pass through and everything else is quoted as a string.
	function literal(v: string, quoted: boolean): string {
		const t = v.trim();
		if (quoted) return JSON.stringify(v);
		if (t === '') return '""';
		if (t === 'true' || t === 'false' || t === 'null') return t;
		if (/^-?\d+(\.\d+)?$/.test(t)) return t;
		return JSON.stringify(t);
	}
	// Parse a literal back to { value, quoted }; quoted strings keep that flag so
	// re-emitting doesn't lose the string type.
	function unliteral(s: string): { value: string; quoted: boolean } {
		if ((s.startsWith('"') && s.endsWith('"')) || (s.startsWith("'") && s.endsWith("'"))) {
			return { value: s.slice(1, -1).replace(/\\(["'\\])/g, '$1'), quoted: true };
		}
		return { value: s, quoted: false };
	}
	function leftOf(r: Row): string {
		const base = r.src === 'flow.input' ? 'flow.input' : `steps.${r.src}.result`;
		const p = r.path.trim().replace(/^\.+/, '');
		return p ? `${base}.${p}` : base;
	}
	function rowExpr(r: Row): string {
		const left = leftOf(r);
		if (r.op === 'true') return left;
		if (r.op === 'false') return `!${left}`;
		return `${left} ${r.op} ${literal(r.value, r.quoted)}`;
	}
	// A row only contributes once it's actually filled in: truthiness ops stand
	// alone, but comparisons need a right-hand value. This keeps a fresh, untouched
	// builder from emitting a phantom `flow.input == ""` (so "empty = always run").
	// An explicit quoted empty string (round-tripped `== ""`) still counts.
	function rowComplete(r: Row): boolean {
		if (r.op === 'true' || r.op === 'false') return true;
		return r.value.trim() !== '' || r.quoted;
	}
	function buildFromRows(rs: Row[]): string {
		return rs
			.filter(rowComplete)
			.map(rowExpr)
			.filter((s) => s.trim() !== '')
			.join(' && ');
	}

	function splitTop(s: string): string[] {
		const out: string[] = [];
		let depth = 0;
		let quote: string | null = null;
		let buf = '';
		for (let i = 0; i < s.length; i++) {
			const c = s[i];
			if (quote) {
				buf += c;
				if (c === quote && s[i - 1] !== '\\') quote = null;
				continue;
			}
			if (c === '"' || c === "'") {
				quote = c;
				buf += c;
				continue;
			}
			if (c === '(' || c === '[') depth++;
			if (c === ')' || c === ']') depth--;
			if (depth === 0 && c === '&' && s[i + 1] === '&') {
				out.push(buf);
				buf = '';
				i++;
				continue;
			}
			buf += c;
		}
		out.push(buf);
		return out;
	}

	function parseLhs(lhs: string): { src: string; path: string } | null {
		let m = lhs.match(/^flow\.input(?:\.(.+))?$/);
		if (m) return { src: 'flow.input', path: m[1] ?? '' };
		m = lhs.match(/^steps\.([A-Za-z0-9_]+)\.result(?:\.(.+))?$/);
		if (m) return { src: m[1], path: m[2] ?? '' };
		return null;
	}
	function parsePiece(p: string): Omit<Row, 'id'> | null {
		const body = p.trim();
		const m = body.match(/^(.*?)\s*(==|!=|<=|>=|<|>)\s*(.+)$/);
		if (m) {
			const sp = parseLhs(m[1].trim());
			if (!sp) return null;
			const { value, quoted } = unliteral(m[3].trim());
			return { ...sp, op: m[2] as Op, value, quoted };
		}
		if (body.startsWith('!')) {
			const sp = parseLhs(body.slice(1).trim());
			return sp ? { ...sp, op: 'false', value: '', quoted: false } : null;
		}
		const sp = parseLhs(body);
		return sp ? { ...sp, op: 'true', value: '', quoted: false } : null;
	}
	/** Returns rows if the expression fits the simple grammar, else null. */
	function parseValue(v: string | undefined): Row[] | null {
		const s = (v ?? '').trim();
		if (s === '') return [];
		const rows: Row[] = [];
		for (const piece of splitTop(s)) {
			const r = parsePiece(piece);
			if (!r) return null;
			rows.push({ id: nextId++, ...r });
		}
		return rows;
	}

	// ---- state ----------------------------------------------------------------
	// The component is keyed per selection in the parent, so `value` is only the
	// initial seed for this instance; read it untracked to make that explicit.
	const initial = untrack(() => parseValue(value));
	let advanced = $state(initial === null);
	// No condition → no rows at all (just the "add condition" button), so the
	// section reads as genuinely empty ("leave empty to always run").
	let rows = $state<Row[]>(initial ?? []);
	let raw = $state(untrack(() => value ?? ''));
	let cannotSimplify = $state(false);

	const builtExpr = $derived(advanced ? raw.trim() : buildFromRows(rows));

	// Source dropdown options: flow.input + upstream steps + any already in use.
	const srcOptions = $derived([
		'flow.input',
		...steps,
		...rows.map((r) => r.src).filter((s) => s !== 'flow.input' && !steps.includes(s))
	]);

	function emit(): void {
		onCommit(builtExpr === '' ? undefined : builtExpr);
	}
	function addRow(): void {
		rows = [...rows, blankRow()];
	}
	function removeRow(id: number): void {
		rows = rows.filter((r) => r.id !== id);
		emit();
	}
	function toAdvanced(): void {
		raw = buildFromRows(rows);
		advanced = true;
		cannotSimplify = false;
	}
	function toSimple(): void {
		const p = parseValue(raw);
		if (p === null) {
			cannotSimplify = true;
			return;
		}
		rows = p;
		advanced = false;
		cannotSimplify = false;
		emit();
	}

	// ---- live validation (debounced) -----------------------------------------
	let validity = $state<'ok' | 'error' | 'checking'>('ok');
	let validityMsg = $state('');
	$effect(() => {
		const expr = builtExpr;
		if (expr === '') {
			validity = 'ok';
			validityMsg = '';
			return;
		}
		validity = 'checking';
		const handle = setTimeout(() => {
			void api
				.forWorkspace(workspace.id)
				.checkExpr(expr)
				.then((r) => {
					validity = r.ok ? 'ok' : 'error';
					validityMsg = r.error ?? '';
				})
				.catch(() => {
					validity = 'ok';
					validityMsg = '';
				});
		}, 400);
		return () => clearTimeout(handle);
	});

	const SELECT =
		'rounded-md border border-border bg-surface-raised px-1.5 py-1 text-xs text-text';
	const INPUT =
		'min-w-0 flex-1 rounded-md border border-border bg-surface-raised px-2 py-1 font-mono text-xs text-text';
</script>

<div class="flex flex-col gap-1.5">
	<div class="flex items-center justify-between">
		<span class="text-xs text-text-tertiary">condition (optional)</span>
		<button
			type="button"
			class="text-[11px] text-accent hover:underline"
			onclick={advanced ? toSimple : toAdvanced}
		>
			{advanced ? 'Simple' : 'Advanced'}
		</button>
	</div>

	{#if advanced}
		<input
			class="{INPUT} w-full"
			placeholder="flow.input.dry_run == true"
			bind:value={raw}
			onchange={emit}
		/>
		{#if cannotSimplify}
			<span class="text-[11px] text-text-tertiary">
				This expression is too complex for the simple builder; edit it here.
			</span>
		{/if}
		<span class="text-[11px] text-text-tertiary">
			Sources: <code>flow.input.*</code>, <code>steps.&lt;id&gt;.result.*</code>. Operators
			<code>== != &lt; &gt; && || !</code>.
		</span>
	{:else}
		{#each rows as row (row.id)}
			<div class="flex flex-col gap-1 rounded-md border border-border bg-surface-alt p-1.5">
				<div class="flex items-center gap-1">
					<select class={SELECT} bind:value={row.src} onchange={emit}>
						{#each srcOptions as opt (opt)}
							<option value={opt}>{opt === 'flow.input' ? 'flow input' : opt}</option>
						{/each}
					</select>
					<input
						class={INPUT}
						placeholder="field e.g. count"
						bind:value={row.path}
						onchange={emit}
					/>
				</div>
				<div class="flex items-center gap-1">
					<select class={SELECT} bind:value={row.op} onchange={emit}>
						{#each OPS as o (o.v)}
							<option value={o.v}>{o.label}</option>
						{/each}
					</select>
					{#if row.op !== 'true' && row.op !== 'false'}
						<input
							class={INPUT}
							placeholder='value e.g. 0, true, "ok"'
							bind:value={row.value}
							onchange={emit}
						/>
					{/if}
					<button
						type="button"
						class="text-text-tertiary hover:text-error"
						title="Remove condition"
						onclick={() => removeRow(row.id)}
					>
						<X size={13} />
					</button>
				</div>
			</div>
		{/each}
		<button type="button" class="flex items-center gap-1 text-[11px] text-accent hover:underline" onclick={addRow}>
			<Plus size={12} /> add condition
		</button>
		{#if rows.length > 1}
			<span class="text-[11px] text-text-tertiary">All conditions must match (AND).</span>
		{/if}
	{/if}

	{#if builtExpr !== ''}
		<div class="flex items-start gap-1 text-[11px]">
			{#if validity === 'ok'}
				<Check size={13} class="mt-px shrink-0 text-success" />
				<code class="break-all text-text-secondary">{builtExpr}</code>
			{:else if validity === 'error'}
				<TriangleAlert size={13} class="mt-px shrink-0 text-error" />
				<span class="break-all text-error">{validityMsg || 'invalid expression'}</span>
			{:else}
				<span class="text-text-tertiary">checking…</span>
			{/if}
		</div>
	{/if}
</div>
