<script lang="ts">
	import {
		SvelteFlow,
		Controls,
		ConnectionMode,
		type Node,
		type Edge,
		type Connection
	} from '@xyflow/svelte';
	import '@xyflow/svelte/dist/style.css';
	import {
		Play,
		Save,
		SlidersHorizontal,
		Trash2,
		Undo2,
		Redo2,
		Pencil,
		ExternalLink,
		CalendarClock
	} from '@lucide/svelte';
	import { onDestroy } from 'svelte';
	import { page } from '$app/state';
	import { goto, replaceState, beforeNavigate } from '$app/navigation';

	import Alert from '$lib/components/common/Alert.svelte';
	import Button from '$lib/components/common/Button.svelte';
	import PageFrame from '$lib/components/common/PageFrame.svelte';
	import FlowStepNode from '$lib/components/flows/FlowStepNode.svelte';
	import ConditionBuilder from '$lib/components/flows/ConditionBuilder.svelte';
	import InputBindings from '$lib/components/flows/InputBindings.svelte';
	import RetryPolicyEditor from '$lib/components/flows/RetryPolicyEditor.svelte';
	import FlowTriggersModal from '$lib/components/triggers/FlowTriggersModal.svelte';
	import AddNodePicker from '$lib/components/flows/AddNodePicker.svelte';
	import {
		specToGraph,
		graphToSpec,
		subtitleFor,
		nodeTitle,
		branchCaseLabel,
		formatCaseValue,
		STEP_NODE_W,
		STEP_NODE_H,
		type StepNodeData
	} from '$lib/services/flow-graph';
	import { api, ApiClientError } from '$lib/services/api';
	import { canWrite } from '$lib/services/permissions';
	import { confirmNavigation } from '$lib/stores/confirm.svelte';
	import { workspace } from '$lib/stores/workspace.svelte';
	import { toastError, toastSuccess } from '$lib/toast';
	import type {
		BranchCase,
		FlowNode,
		FlowSpec,
		FlowRunState,
		NodeBody,
		InputBinding,
		RetryPolicy,
		ScriptListItem,
		UserInfo,
		TriggerRule
	} from '$lib/types';

	const nodeTypes = { step: FlowStepNode };

	const FIELD = 'rounded-md border border-border bg-surface-raised px-2 py-1.5 text-sm text-text';
	const ARGS_PLACEHOLDER = '{ "dry_run": true }';

	const initialPath = $derived(page.params.path ?? '');
	const isNew = $derived(initialPath === '');

	let pathInput = $state('');
	// New flow opened from a folder's "+ new" → prefill the path with that folder.
	// Re-syncs on each navigation so a different folder (or a blank New) updates it.
	$effect(() => {
		if (!isNew) return;
		const folder = page.url.searchParams.get('folder');
		pathInput = folder ? `${folder}/` : '';
	});
	let loaded = $state(false);
	let loadError = $state<string | null>(null);
	let busy = $state(false);

	let nodes = $state.raw<Node[]>([]);
	let edges = $state.raw<Edge[]>([]);
	let selectedNodeId = $state<string | null>(null);
	let selectedEdgeId = $state<string | null>(null);
	// Canvas viewport + size, so a newly-added node lands where the user is looking.
	let viewport = $state({ x: 0, y: 0, zoom: 1 });
	let canvasW = $state(0);
	let canvasH = $state(0);
	// Resizable right-hand config panel.
	let configWidth = $state(320);
	let resizingConfig = $state(false);
	let resizeStartX = 0;
	let resizeStartW = 0;

	let scripts = $state<ScriptListItem[]>([]);
	// Only true once the script list actually loaded; gates the broken-node check
	// so a failed listScripts fetch doesn't falsely flag every script node.
	let scriptsLoaded = $state(false);

	// Flows reference scripts by stable script_id; resolve to the current item for
	// display (path/name) since the path is a movable label.
	const scriptsById = $derived.by(() => {
		const m: Record<string, ScriptListItem> = {};
		for (const s of scripts) m[s.script_id] = s;
		return m;
	});
	function scriptLabel(id: string): string | undefined {
		return scriptsById[id]?.path;
	}
	// Resolver passed to specToGraph: undefined until scripts load, so nodes are
	// never marked broken on a transient fetch failure.
	const labelResolver = $derived(scriptsLoaded ? scriptLabel : undefined);

	// Undo/redo history of FlowSpec snapshots (positions included).
	let undoStack = $state<FlowSpec[]>([]);
	let redoStack = $state<FlowSpec[]>([]);
	// Unsaved-changes flag: set on any edit, cleared on save/load.
	let dirty = $state(false);

	// Run-in-place: after Save & run we stay on the canvas and overlay live status.
	let mode = $state<'edit' | 'run'>('edit');
	let runId = $state<string | null>(null);
	let runStatus = $state<string>('');
	let nodeRunIds = $state<Record<string, string>>({});
	let pollTimer: ReturnType<typeof setInterval> | undefined;

	// Optional flow.input parameters (JSON) supplied at run time.
	let showParams = $state(false);
	let argsText = $state('');

	// Flow-level settings (not represented on the canvas): max concurrent nodes,
	// and an on-error handler node. Kept as editor state and folded back into the
	// spec on save / undo so they aren't lost on a round-trip.
	let maxConcurrent = $state('');
	let onError = $state<FlowNode | null>(null);
	let flowRetry = $state<RetryPolicy | null>(null);
	// Schedules for this flow are managed here (no standalone page). Anyone who
	// can open the flow (read) may view its schedules; create/edit/delete require
	// write (canManageSchedules, mirrors the backend).
	let scheduleOpen = $state(false);
	// Stable id of the loaded flow (schedules reference flows by id, not path).
	let flowId = $state('');
	let me = $state<UserInfo | null>(null);
	let meLoadedWs: string | null = null;
	$effect(() => {
		const ws = workspace.id;
		if (!ws || meLoadedWs === ws) return;
		meLoadedWs = ws;
		api
			.forWorkspace(ws)
			.getMe()
			.then((m) => (me = m))
			.catch(() => {});
	});
	const canManageSchedules = $derived(canWrite(initialPath, me));

	// --- load --------------------------------------------------------------
	let lastLoaded: string | null = null;
	$effect(() => {
		const p = initialPath;
		const wsId = workspace.id;
		if (lastLoaded === `${wsId}::${p}`) return;
		lastLoaded = `${wsId}::${p}`;
		void (async () => {
			try {
				scripts = await api.forWorkspace(wsId).listScripts();
				scriptsLoaded = true;
			} catch {
				/* palette is best-effort */
			}
			if (p === '') {
				rebuild(specToGraph({ nodes: [], edges: [] }));
				applyMeta({ nodes: [], edges: [] });
				loaded = true;
				dirty = false;
				return;
			}
			pathInput = p;
			try {
				const flow = await api.forWorkspace(wsId).getFlow(p);
				if (workspace.id !== wsId) return;
				flowId = flow.flow_id;
				rebuild(specToGraph(flow.value, undefined, labelResolver));
				applyMeta(flow.value);
				loaded = true;
				dirty = false;
				// Resume the live run overlay when returning here with ?run=<id>
				// (e.g. pressing Back from a node's run-detail page).
				const runParam = page.url.searchParams.get('run');
				if (runParam) {
					runId = runParam;
					mode = 'run';
					startPoll();
				}
			} catch (e) {
				if (workspace.id !== wsId) return;
				loadError = e instanceof ApiClientError ? `${e.status}: ${e.body || e.message}` : String(e);
				loaded = true;
			}
		})();
	});

	// Inject interactive callbacks + selection flag into each node's data.
	function rebuild(g: { nodes: Node[]; edges: Edge[] }): void {
		edges = g.edges;
		nodes = g.nodes.map(withCallbacks);
	}
	function withCallbacks(n: Node): Node {
		const data = n.data as StepNodeData;
		return {
			...n,
			data: {
				...data,
				selected: data.node.id === selectedNodeId,
				onSelect: selectNode,
				onDelete: deleteNode
			}
		};
	}
	function refreshNodes(): void {
		nodes = nodes.map(withCallbacks);
	}

	function selectNode(id: string): void {
		selectedNodeId = id;
		selectedEdgeId = null;
		refreshNodes();
	}
	// Clicking empty canvas clears the selection (closes the NODE/Edge panel).
	function clearSelection(): void {
		selectedNodeId = null;
		selectedEdgeId = null;
		refreshNodes();
	}
	function deleteNode(id: string): void {
		snapshot();
		nodes = nodes.filter((n) => n.id !== id);
		edges = edges.filter((e) => e.source !== id && e.target !== id);
		if (selectedNodeId === id) selectedNodeId = null;
	}

	const selectedNode = $derived(
		selectedNodeId
			? ((nodes.find((n) => n.id === selectedNodeId)?.data as StepNodeData)?.node ?? null)
			: null
	);
	const selectedEdge = $derived(
		selectedEdgeId ? (edges.find((e) => e.id === selectedEdgeId) ?? null) : null
	);
	// Edges out of a Branch are routed by case, so the edge panel swaps the
	// `when` builder for a case editor.
	const selectedEdgeIsBranch = $derived(
		selectedEdge ? nodeKindOf(selectedEdge.source) === 'branch' : false
	);
	const selectedEdgeCase = $derived(
		(selectedEdge?.data as { case?: BranchCase } | undefined)?.case
	);
	// The selected Branch node's outgoing edges, summarised for the node panel.
	const branchCases = $derived.by(() => {
		const n = selectedNode;
		if (!n || n.body.kind !== 'branch') return [];
		return edges
			.filter((e) => e.source === n.id)
			.map((e) => ({ to: e.target, case: (e.data as { case?: BranchCase } | undefined)?.case }));
	});

	// --- add nodes ---------------------------------------------------------
	function uniqueId(base: string): string {
		const clean = base.replace(/[^a-zA-Z0-9_]/g, '_').replace(/^_+|_+$/g, '') || 'node';
		const taken = new Set(nodes.map((n) => n.id));
		if (!taken.has(clean)) return clean;
		let i = 2;
		while (taken.has(`${clean}_${i}`)) i++;
		return `${clean}_${i}`;
	}
	// Place a new node at the centre of the current viewport (with a small
	// per-add offset so successive adds don't stack exactly) — so it's always
	// visible and the user sees the click took effect, regardless of pan/zoom.
	function newNodePos(): { x: number; y: number } {
		const k = nodes.length % 6;
		if (canvasW > 0 && canvasH > 0 && viewport.zoom > 0) {
			const cx = (canvasW / 2 - viewport.x) / viewport.zoom;
			const cy = (canvasH / 2 - viewport.y) / viewport.zoom;
			return {
				x: Math.round(cx - STEP_NODE_W / 2 + k * 28),
				y: Math.round(cy - STEP_NODE_H / 2 + k * 28)
			};
		}
		return { x: 80 + (nodes.length % 3) * 280, y: 60 + Math.floor(nodes.length / 3) * 120 };
	}
	function addNode(body: NodeBody, idBase: string): void {
		snapshot();
		const id = uniqueId(idBase);
		const node: FlowNode = { id, body };
		const pos = newNodePos();
		const data: StepNodeData = {
			node,
			title: nodeTitle(node, scriptLabel),
			subtitle: subtitleFor(body, scriptLabel),
			status: 'idle',
			onSelect: selectNode,
			onDelete: deleteNode
		};
		nodes = [...nodes, { id, type: 'step', position: pos, data }];
		selectNode(id);
	}
	// Cache of script_id -> its required main() param names (from script.schema).
	const requiredCache = new Map<string, string[]>();
	async function requiredParams(scriptId: string): Promise<string[]> {
		const cached = requiredCache.get(scriptId);
		if (cached) return cached;
		const path = scriptsById[scriptId]?.path;
		if (!path) return [];
		try {
			const script = await api.forWorkspace(workspace.id).getScriptByPath(path);
			const req = (script.schema as { required?: string[] } | null)?.required;
			const names = Array.isArray(req) ? req : [];
			requiredCache.set(scriptId, names);
			return names;
		} catch {
			return [];
		}
	}
	// Detect button: fetch the selected script's schema fresh and give explicit
	// feedback (the prefill path stays silent). A null schema means the script
	// couldn't be parsed (syntax error / no main()) or predates schema support.
	async function detectInputs(): Promise<string[]> {
		const body = selectedNode?.body;
		// A Branch's operator is its `task` script.
		const leaf = body?.kind === 'branch' ? body.task : body;
		if (!leaf || leaf.kind !== 'script') return [];
		const path = scriptsById[leaf.script_id]?.path;
		if (!path) {
			toastError('Referenced script not found in this workspace.');
			return [];
		}
		try {
			const script = await api.forWorkspace(workspace.id).getScriptByPath(path);
			const schema = script.schema as { required?: string[] } | null;
			if (schema == null) {
				toastError(
					'No parameter schema for this script — it may have a syntax error or no main(). Open it on the Scripts page and re-save.'
				);
				return [];
			}
			const req = Array.isArray(schema.required) ? schema.required : [];
			requiredCache.set(leaf.script_id, req);
			if (req.length === 0) toastSuccess('main() has no required parameters.');
			return req;
		} catch {
			toastError('Could not load the script.');
			return [];
		}
	}

	// Reference the script by its stable id; the node label is the script's
	// current name. Required params are prefilled as empty expression bindings.
	async function scriptBodyFor(s: ScriptListItem): Promise<NodeBody> {
		const req = await requiredParams(s.script_id);
		const inputs: Record<string, InputBinding> | undefined = req.length
			? Object.fromEntries(req.map((n) => [n, { kind: 'expr', expr: '' }]))
			: undefined;
		return { kind: 'script', script_id: s.script_id, inputs };
	}
	function scriptLeaf(s: ScriptListItem): string {
		return s.name || s.path.split('/').pop() || s.path;
	}
	async function addScript(s: ScriptListItem): Promise<void> {
		addNode(await scriptBodyFor(s), scriptLeaf(s));
	}
	// A Branch wraps a script as its operator `task`; it runs once and its result
	// routes the outgoing edges (see the edge panel's case editor).
	async function addBranch(s: ScriptListItem): Promise<void> {
		addNode({ kind: 'branch', task: await scriptBodyFor(s) }, `branch_${scriptLeaf(s)}`);
	}

	// --- edges -------------------------------------------------------------
	// Forward reachability over current edges (for cycle prevention).
	function reaches(from: string, to: string): boolean {
		const adj = new Map<string, string[]>();
		for (const e of edges) {
			const arr = adj.get(e.source) ?? [];
			arr.push(e.target);
			adj.set(e.source, arr);
		}
		const seen = new Set<string>();
		const stack = [from];
		while (stack.length) {
			const n = stack.pop();
			if (n === undefined) continue;
			if (n === to) return true;
			if (seen.has(n)) continue;
			seen.add(n);
			for (const m of adj.get(n) ?? []) stack.push(m);
		}
		return false;
	}
	function onConnect(c: Connection): void {
		if (!c.source || !c.target || c.source === c.target) return;
		// ConnectionMode.Loose lets a drag start on a target handle (top/left),
		// which would otherwise serialize a reversed dependency. Orient the edge by
		// handle type: t/l are target handles, b/r are source handles.
		const isTargetHandle = (h?: string | null) => h === 't' || h === 'l';
		let source = c.source;
		let target = c.target;
		let sourceHandle = c.sourceHandle;
		let targetHandle = c.targetHandle;
		if (isTargetHandle(sourceHandle)) {
			[source, target] = [target, source];
			[sourceHandle, targetHandle] = [targetHandle, sourceHandle];
		}
		if (source === target) return;
		if (edges.some((e) => e.source === source && e.target === target)) return;
		// Reject edges that would introduce a cycle (target already reaches source).
		if (reaches(target, source)) {
			toastError('That connection would create a cycle.');
			return;
		}
		snapshot();
		edges = [
			...edges,
			{
				id: `e-${source}-${target}`,
				source,
				target,
				sourceHandle,
				targetHandle,
				type: 'smoothstep'
			}
		];
	}

	// --- edit selected node ------------------------------------------------
	function updateNode(mut: (n: FlowNode) => FlowNode): void {
		const id = selectedNodeId;
		if (!id) return;
		snapshot();
		nodes = nodes.map((n) => {
			if (n.id !== id) return n;
			const data = n.data as StepNodeData;
			const next = mut(data.node);
			return {
				...n,
				data: {
					...data,
					node: next,
					title: nodeTitle(next, scriptLabel),
					subtitle: subtitleFor(next.body, scriptLabel)
				}
			};
		});
	}
	function deleteSelectedEdge(): void {
		snapshot();
		edges = edges.filter((e) => e.id !== selectedEdgeId);
		selectedEdgeId = null;
	}

	// --- branch routing ----------------------------------------------------
	function nodeKindOf(id: string): NodeBody['kind'] | undefined {
		const n = nodes.find((x) => x.id === id);
		return n ? (n.data as StepNodeData).node.body.kind : undefined;
	}
	// Parse a case value: try JSON (so `42`/`true` become typed) else a string.
	function parseCaseValue(raw: string): unknown {
		try {
			return JSON.parse(raw.trim());
		} catch {
			return raw;
		}
	}
	function setEdgeCase(c: BranchCase | undefined): void {
		const id = selectedEdgeId;
		snapshot();
		edges = edges.map((ed) =>
			ed.id === id ? { ...ed, data: { case: c }, label: branchCaseLabel(c) } : ed
		);
	}
	// Convert the selected node between a plain Script and a Branch (wrapping or
	// unwrapping its script task) — the underlying Type switch.
	function setNodeKind(kind: 'script' | 'branch'): void {
		const id = selectedNodeId;
		const cur = selectedNode?.body;
		if (!id || !cur) return;
		// Decide the conversion up front; bail (without touching edges) if it isn't
		// a valid Script<->Branch swap, so a no-op click never wipes edge routing.
		let nextBody: NodeBody | null = null;
		if (kind === 'branch' && cur.kind === 'script') {
			nextBody = { kind: 'branch', task: cur };
		} else if (kind === 'script' && cur.kind === 'branch' && cur.task.kind === 'script') {
			nextBody = cur.task;
		}
		if (!nextBody) return;
		const body = nextBody;
		updateNode((nd) => ({ ...nd, body }));
		// Switching Script<->Branch flips whether outgoing edges route by `when` or
		// by `case`; clear the now-irrelevant value so a stale one can't be saved.
		edges = edges.map((e) => (e.source === id ? { ...e, label: undefined, data: undefined } : e));
	}

	// --- resizable config panel -------------------------------------------
	function startConfigResize(e: PointerEvent): void {
		if (e.button !== 0) return;
		resizingConfig = true;
		resizeStartX = e.clientX;
		resizeStartW = configWidth;
		(e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
		e.preventDefault();
	}
	function moveConfigResize(e: PointerEvent): void {
		if (!resizingConfig) return;
		// The panel is on the right, so dragging left (smaller clientX) widens it.
		configWidth = Math.min(640, Math.max(248, resizeStartW + (resizeStartX - e.clientX)));
	}
	function endConfigResize(e: PointerEvent): void {
		if (!resizingConfig) return;
		resizingConfig = false;
		const t = e.currentTarget as HTMLElement;
		if (t.hasPointerCapture(e.pointerId)) t.releasePointerCapture(e.pointerId);
	}
	// Upstream step ids reachable backwards from a node — the only results that
	// can be referenced in that node's expressions.
	function ancestorIds(id: string): string[] {
		const incoming = new Map<string, string[]>();
		for (const e of edges) {
			const arr = incoming.get(e.target) ?? [];
			arr.push(e.source);
			incoming.set(e.target, arr);
		}
		const seen = new Set<string>();
		const stack = [...(incoming.get(id) ?? [])];
		while (stack.length) {
			const n = stack.pop();
			if (n === undefined || seen.has(n)) continue;
			seen.add(n);
			for (const p of incoming.get(n) ?? []) stack.push(p);
		}
		return [...seen];
	}

	// --- undo / redo -------------------------------------------------------
	// --- flow-level settings (max_concurrent, on_error) --------------------
	// Build the meta from editor state. Omit max_concurrent unless it's a valid
	// positive int (the backend rejects 0; empty = unlimited).
	function flowMeta(): { max_concurrent?: number; on_error?: FlowNode; retry?: RetryPolicy } {
		const meta: { max_concurrent?: number; on_error?: FlowNode; retry?: RetryPolicy } = {};
		const mc = Number(maxConcurrent);
		if (maxConcurrent.trim() !== '' && Number.isInteger(mc) && mc > 0) meta.max_concurrent = mc;
		if (onError) meta.on_error = onError;
		if (flowRetry) meta.retry = flowRetry;
		return meta;
	}
	// The full spec for the current canvas + flow settings.
	function currentSpec(): FlowSpec {
		return graphToSpec(nodes, edges, flowMeta());
	}
	// Restore the flow-settings editor state from a spec (load / undo / redo).
	function applyMeta(spec: FlowSpec): void {
		maxConcurrent = spec.max_concurrent != null ? String(spec.max_concurrent) : '';
		onError = spec.on_error ?? null;
		flowRetry = spec.retry ?? null;
	}
	// Snapshot-then-mutate wrapper for flow-settings edits (so undo covers them).
	function editFlowSettings(fn: () => void): void {
		snapshot();
		fn();
	}

	const MAX_HISTORY = 50;
	// Capture the pre-change state; call at the start of each mutating action.
	function snapshot(): void {
		undoStack = [...undoStack, currentSpec()].slice(-MAX_HISTORY);
		redoStack = [];
		dirty = true;
	}
	function undo(): void {
		const prev = undoStack.at(-1);
		if (!prev) return;
		redoStack = [...redoStack, currentSpec()];
		undoStack = undoStack.slice(0, -1);
		selectedNodeId = null;
		selectedEdgeId = null;
		dirty = true;
		applyMeta(prev);
		rebuild(specToGraph(prev, undefined, labelResolver));
	}
	function redo(): void {
		const next = redoStack.at(-1);
		if (!next) return;
		undoStack = [...undoStack, currentSpec()];
		redoStack = redoStack.slice(0, -1);
		selectedNodeId = null;
		selectedEdgeId = null;
		dirty = true;
		applyMeta(next);
		rebuild(specToGraph(next, undefined, labelResolver));
	}

	// --- run in place (overlay live status on the canvas) ------------------
	function isTerminal(s: string): boolean {
		return s === 'success' || s === 'failure' || s === 'cancelled';
	}
	function applyRunState(fs: FlowRunState): void {
		const byId = new Map(fs.nodes.map((s) => [s.id, s]));
		const runIds: Record<string, string> = {};
		for (const s of fs.nodes) {
			if ('run_id' in s && s.run_id) {
				runIds[s.id] = s.run_id;
			} else if ('fanout' in s && s.fanout?.children?.length) {
				// Map nodes have no top-level run_id; drill into the first child run.
				runIds[s.id] = s.fanout.children[0];
			}
		}
		nodeRunIds = runIds;
		nodes = nodes.map((n) => {
			const data = n.data as StepNodeData;
			const st = byId.get(n.id);
			return {
				...n,
				data: { ...data, status: st?.state ?? 'idle', selected: false, onSelect: openNodeLog, onDelete: undefined }
			};
		});
		// Colour edges by the run state of their endpoints, matching the node pills:
		//   completed path (src+tgt succeeded) → green
		//   in progress  (src succeeded, tgt running) → accent + animated
		//   failed target → red
		//   otherwise → default (untravelled)
		edges = edges.map((e) => {
			const src = byId.get(e.source)?.state;
			const tgt = byId.get(e.target)?.state;
			let stroke: string | undefined;
			let animated = false;
			if (src === 'succeeded' && tgt === 'succeeded') {
				stroke = 'var(--color-success)';
			} else if (src === 'succeeded' && tgt === 'running') {
				stroke = 'var(--color-accent)';
				animated = true;
			} else if (tgt === 'failed') {
				stroke = 'var(--color-error)';
			}
			return {
				...e,
				animated,
				style: stroke ? `stroke: ${stroke}; stroke-width: 2;` : undefined
			};
		});
	}
	function openNodeLog(id: string): void {
		const child = nodeRunIds[id];
		if (!child) return;
		// Build the return URL from `runId` directly (mirrors setRunParam) rather
		// than reading page.url — the `?run=` set via shallow routing (replaceState)
		// isn't reliably reflected there, which would drop it and land Back in edit
		// mode instead of resuming the live run overlay.
		const path = (isNew ? pathInput : initialPath).trim();
		const enc = path.split('/').map(encodeURIComponent).join('/');
		const back = `/flows/edit/${enc}${runId ? `?run=${runId}` : ''}`;
		void goto(`/runs/${child}?from=${encodeURIComponent(back)}`);
	}
	// Guard against overlapping requests on a slow network: skip a tick if the
	// previous fetch is still in flight.
	let polling = false;
	async function poll(): Promise<void> {
		if (!runId || polling) return;
		polling = true;
		try {
			const run = await api.forWorkspace(workspace.id).getRun(runId);
			runStatus = run.status;
			if (run.flow_status) applyRunState(run.flow_status);
			if (isTerminal(run.status)) stopPoll();
		} catch {
			/* transient; next tick retries */
		} finally {
			polling = false;
		}
	}
	function startPoll(): void {
		stopPoll();
		void poll();
		pollTimer = setInterval(() => void poll(), 1000);
	}
	function stopPoll(): void {
		if (pollTimer) clearInterval(pollTimer);
		pollTimer = undefined;
	}
	function backToEdit(): void {
		stopPoll();
		mode = 'edit';
		runId = null;
		runStatus = '';
		nodeRunIds = {};
		setRunParam(null);
		// Reset statuses to idle + rewire edit callbacks by round-tripping the spec.
		rebuild(specToGraph(graphToSpec(nodes, edges), undefined, scriptLabel));
	}

	// Reflect the active run in the URL (shallow routing) so navigating to a
	// node's run-detail page and pressing Back returns here and resumes.
	function setRunParam(id: string | null): void {
		const path = (isNew ? pathInput : initialPath).trim();
		if (!path) return;
		const enc = path.split('/').map(encodeURIComponent).join('/');
		replaceState(id ? `/flows/edit/${enc}?run=${id}` : `/flows/edit/${enc}`, {});
	}
	onDestroy(stopPoll);

	// Warn before leaving with unsaved edits. beforeNavigate covers in-app
	// (SvelteKit) navigation; onbeforeunload covers tab close / reload. Shallow
	// run-overlay URL updates (replaceState) don't trigger beforeNavigate.
	beforeNavigate((nav) => {
		// confirmNavigation no-ops on full-page unload (onbeforeunload owns that).
		if (dirty) {
			confirmNavigation(nav, {
				title: 'Leave without saving?',
				message: 'You have unsaved changes to this flow.',
				confirmLabel: 'Leave',
				variant: 'danger'
			});
		}
	});
	function onBeforeUnload(e: BeforeUnloadEvent): void {
		if (dirty) {
			e.preventDefault();
			e.returnValue = '';
		}
	}

	// Editor keyboard shortcuts. Skipped while typing in a form field (so native
	// text editing / undo still works) and while watching a run.
	function onKeydown(e: KeyboardEvent): void {
		if (mode === 'run') return;
		const tag = (e.target as HTMLElement)?.tagName;
		const inField = tag === 'INPUT' || tag === 'TEXTAREA';
		if (inField) return;
		if ((e.metaKey || e.ctrlKey) && (e.key === 'z' || e.key === 'Z')) {
			e.preventDefault();
			if (e.shiftKey) redo();
			else undo();
			return;
		}
		if (e.key === 'Delete' || e.key === 'Backspace') {
			if (selectedEdgeId) {
				e.preventDefault();
				deleteSelectedEdge();
			} else if (selectedNodeId) {
				e.preventDefault();
				deleteNode(selectedNodeId);
			}
		}
	}

	// --- save / run --------------------------------------------------------
	async function save(): Promise<string | null> {
		const path = (isNew ? pathInput : initialPath).trim();
		if (!path) {
			toastError('Path is required');
			return null;
		}
		if (path.endsWith('/')) {
			toastError('Flow name is required — add a name after the folder (e.g. users/you/my-flow)');
			return null;
		}
		if (nodes.length === 0) {
			toastError('Add at least one node');
			return null;
		}
		busy = true;
		try {
			await api.forWorkspace(workspace.id).createFlow({ path, value: currentSpec() });
			dirty = false;
			toastSuccess('Flow saved');
			return path;
		} catch (e) {
			toastError(e instanceof ApiClientError ? `${e.status}: ${e.body || e.message}` : String(e));
			return null;
		} finally {
			busy = false;
		}
	}
	async function onSave(): Promise<void> {
		const path = await save();
		if (path && isNew) void goto(`/flows/edit/${path.split('/').map(encodeURIComponent).join('/')}`);
	}
	async function run(): Promise<void> {
		// Parse optional flow.input parameters before saving, so a bad JSON blob
		// doesn't leave us half-committed.
		let args: unknown;
		const trimmed = argsText.trim();
		if (trimmed !== '') {
			try {
				args = JSON.parse(trimmed);
			} catch {
				toastError('Run parameters must be valid JSON');
				return;
			}
		}
		// Only save when there are unsaved edits (or it's a brand-new flow);
		// otherwise just run the already-saved definition.
		const path = dirty || isNew ? await save() : initialPath.trim();
		if (!path) return;
		busy = true;
		try {
			const { id } = await api.forWorkspace(workspace.id).runFlow(path, args);
			// Stay on the canvas and overlay live status instead of navigating away.
			// Click a node to jump to its detailed log.
			runId = id;
			mode = 'run';
			clearSelection();
			startPoll();
			setRunParam(id);
		} catch (e) {
			toastError(e instanceof ApiClientError ? `${e.status}: ${e.body || e.message}` : String(e));
		} finally {
			busy = false;
		}
	}
</script>

<svelte:head>
	<title>{isNew ? 'New flow' : initialPath} | CoveFlow</title>
</svelte:head>

<svelte:window onkeydown={onKeydown} onbeforeunload={onBeforeUnload} />

<PageFrame title={isNew ? 'New flow' : initialPath} subtitle="Add scripts and connect them into a flow">
	{#snippet actions()}
		{#if mode === 'run'}
			<span
				class="inline-flex items-center gap-1.5 rounded-full px-3 py-1 text-xs font-medium
				{runStatus === 'success'
					? 'bg-success/15 text-success'
					: runStatus === 'failure'
						? 'bg-error/15 text-error'
						: 'bg-accent/15 text-accent'}"
			>
				{#if !isTerminal(runStatus)}
					<span class="h-1.5 w-1.5 animate-pulse rounded-full bg-current"></span>
				{/if}
				{runStatus || 'starting'}
			</span>
			<Button onclick={() => runId && goto(`/runs/${runId}`)}>
				<ExternalLink size={16} /> Run page
			</Button>
			<Button variant="primary" onclick={backToEdit}>
				<Pencil size={16} /> Edit
			</Button>
		{:else}
			<Button onclick={undo} disabled={busy || undoStack.length === 0} aria-label="Undo">
				<Undo2 size={16} />
			</Button>
			<Button onclick={redo} disabled={busy || redoStack.length === 0} aria-label="Redo">
				<Redo2 size={16} />
			</Button>
			<Button onclick={() => (showParams = !showParams)} disabled={busy}>
				<SlidersHorizontal size={16} /> Params
			</Button>
			{#if !isNew}
				<Button onclick={() => (scheduleOpen = true)} disabled={busy}>
					<CalendarClock size={16} /> Triggers
				</Button>
			{/if}
			<Button onclick={onSave} disabled={busy || !dirty}>
				<Save size={16} /> {dirty ? 'Save' : 'Saved'}
			</Button>
			<Button variant="primary" onclick={run} disabled={busy || nodes.length === 0}>
				<Play size={16} /> {dirty || isNew ? 'Save & run' : 'Run'}
			</Button>
		{/if}
	{/snippet}

	{#if loadError}
		<Alert variant="error">{loadError}</Alert>
	{:else if loaded}
		{#if isNew}
			<input
				class="mb-3 w-full max-w-md rounded-md border border-border bg-surface px-3 py-2 text-sm"
				placeholder="flow path, e.g. workspace/etl/daily"
				bind:value={pathInput}
			/>
		{/if}
		{#if showParams && mode === 'edit'}
			<div class="mb-3 flex flex-col gap-1 rounded-md border border-border bg-surface p-3">
				<span class="text-xs font-medium text-text">Run parameters (JSON)</span>
				<textarea
					class="h-24 w-full resize-y rounded-md border border-border bg-surface-raised px-2 py-1.5 font-mono text-xs text-text"
					placeholder={ARGS_PLACEHOLDER}
					bind:value={argsText}
				></textarea>
				<span class="text-[11px] text-text-tertiary">
					Passed to the run as <code>flow.input</code>. Reference fields in conditions/inputs as
					<code>flow.input.&lt;field&gt;</code>.
				</span>
			</div>
		{/if}
		<div class="flex h-[calc(100svh-200px)] gap-3">
			<!-- Canvas. Nodes are added via the "Add node" search popover overlaid
			     top-left (no separate script tree — the workspace explorer on the
			     left of the page is the single file navigator). -->
			<div
				class="relative min-w-0 flex-1 overflow-hidden rounded-md border border-border bg-surface-sunken"
				bind:clientWidth={canvasW}
				bind:clientHeight={canvasH}
			>
				{#if mode !== 'run'}
					<div class="absolute top-3 left-3 z-10">
						<AddNodePicker
							{scripts}
							onAdd={(s, asBranch) => (asBranch ? addBranch(s) : addScript(s))}
						/>
					</div>
				{/if}
				<SvelteFlow
					bind:nodes
					bind:edges
					bind:viewport
					{nodeTypes}
					fitView
					fitViewOptions={{ maxZoom: 1, padding: 0.25 }}
					maxZoom={1.5}
					minZoom={0.4}
					connectionMode={ConnectionMode.Loose}
					onconnect={onConnect}
					onnodedragstart={snapshot}
					onedgeclick={({ edge }) => {
						selectedEdgeId = edge.id;
						selectedNodeId = null;
						refreshNodes();
					}}
					onpaneclick={clearSelection}
					proOptions={{ hideAttribution: true }}
				>
					<Controls showLock={false} />
				</SvelteFlow>
			</div>

			<!-- Drag handle to resize the config panel (hidden while watching a run) -->
			<div
				role="separator"
				aria-orientation="vertical"
				aria-label="Resize config panel"
				title="Drag to resize"
				class="group flex w-1.5 shrink-0 cursor-col-resize touch-none items-center justify-center rounded bg-border transition hover:bg-accent/30 {resizingConfig
					? 'bg-accent/40'
					: ''}"
				class:hidden={mode === 'run'}
				onpointerdown={startConfigResize}
				onpointermove={moveConfigResize}
				onpointerup={endConfigResize}
				onpointercancel={endConfigResize}
				onlostpointercapture={endConfigResize}
			>
				<span class="h-10 w-px rounded-full bg-border-strong transition group-hover:bg-accent"></span>
			</div>

			<!-- Config panel (hidden while watching a run) -->
			<aside
				class="flex shrink-0 flex-col gap-3 overflow-y-auto rounded-md border border-border bg-surface p-3 text-sm"
				class:hidden={mode === 'run'}
				style="width: {configWidth}px"
			>
				{#if selectedNode}
					{@const n = selectedNode}
					<div class="flex items-center justify-between">
						<h3 class="text-xs font-semibold tracking-wide text-text-tertiary uppercase">Node</h3>
						<button class="text-text-tertiary hover:text-error" title="Delete" onclick={() => deleteNode(n.id)}>
							<Trash2 size={14} />
						</button>
					</div>
					<div class="flex flex-col gap-1">
						<span class="text-xs text-text-tertiary">name</span>
						<div class="rounded-md border border-border bg-surface-alt px-2 py-1.5">
							<span class="text-sm font-medium text-text">{n.id}</span>
						</div>
						<span class="text-[11px] text-text-tertiary">
							reference its output as <code class="text-text-secondary">steps.{n.id}.result</code>
						</span>
					</div>

					{#if n.body.kind === 'script' || n.body.kind === 'branch'}
						<div class="flex flex-col gap-1">
							<span class="text-xs text-text-tertiary">type</span>
							<div class="flex gap-1">
								{#each [['script', 'Script'], ['branch', 'Branch']] as const as [kind, label] (kind)}
									<button
										type="button"
										class="flex-1 rounded-md border px-2 py-1 text-xs font-medium transition-colors {n.body
											.kind === kind
											? 'border-accent bg-accent/10 text-accent'
											: 'border-border text-text-secondary hover:bg-surface-alt'}"
										onclick={() => setNodeKind(kind)}
									>
										{label}
									</button>
								{/each}
							</div>
						</div>
					{/if}

					{#if n.body.kind === 'script'}
						<div class="flex flex-col gap-1">
							<span class="text-xs text-text-tertiary">script</span>
							<div class="rounded-md border border-border bg-surface-alt px-2 py-1.5">
								<span class="font-mono text-sm text-text-secondary">
									{scriptsById[n.body.script_id]?.path ?? '(script not found)'}
								</span>
							</div>
							<span class="text-[11px] text-text-tertiary">
								Authored on the Scripts page — the flow references it by id, so moving it
								won't break this node.
							</span>
						</div>
					{:else if n.body.kind === 'branch'}
						<div class="flex flex-col gap-1">
							<span class="text-xs text-text-tertiary">branch operator (script)</span>
							<div class="rounded-md border border-border bg-surface-alt px-2 py-1.5">
								<span class="font-mono text-sm text-text-secondary">
									{n.body.task.kind === 'script'
										? (scriptsById[n.body.task.script_id]?.path ?? '(script not found)')
										: n.body.task.kind}
								</span>
							</div>
							<span class="text-[11px] text-text-tertiary">
								Runs once; its return value (a key or array of keys) selects which outgoing edges
								run. Set each edge's case below.
							</span>
						</div>
						<div class="flex flex-col gap-1">
							<span class="text-xs text-text-tertiary">routes</span>
							{#if branchCases.length === 0}
								<span class="text-[11px] text-text-tertiary">
									No outgoing edges yet — connect this node to targets, then click each edge to set
									its case.
								</span>
							{:else}
								<ul class="flex flex-col gap-0.5">
									{#each branchCases as c (c.to)}
										<li
											class="flex items-center justify-between rounded border border-border bg-surface-alt px-2 py-1 text-xs"
										>
											<span class="font-mono text-text-secondary">
												{branchCaseLabel(c.case) ?? '(no case)'}
											</span>
											<span class="text-text-tertiary">→ {c.to}</span>
										</li>
									{/each}
								</ul>
							{/if}
						</div>
					{/if}

					{#if n.body.kind === 'script'}
						{#key n.id}
							<InputBindings
								inputs={n.body.inputs}
								steps={ancestorIds(n.id)}
								onCommit={(inputs) =>
									updateNode((nd) => ({ ...nd, body: { ...nd.body, inputs } as NodeBody }))}
								onDetect={detectInputs}
							/>
						{/key}
					{:else if n.body.kind === 'branch' && n.body.task.kind === 'script'}
						{#key n.id}
							<InputBindings
								inputs={n.body.task.inputs}
								steps={ancestorIds(n.id)}
								onCommit={(inputs) =>
									updateNode((nd) =>
										nd.body.kind === 'branch'
											? { ...nd, body: { ...nd.body, task: { ...nd.body.task, inputs } } }
											: nd
									)}
								onDetect={detectInputs}
							/>
						{/key}
					{/if}

					{#if n.body.kind === 'script' || n.body.kind === 'branch'}
						<div class="flex flex-col gap-1">
							<span class="text-xs text-text-tertiary">retry on failure (overrides flow default)</span>
							{#key n.id}
								<RetryPolicyEditor
									value={n.retry}
									onCommit={(retry) => updateNode((nd) => ({ ...nd, retry }))}
								/>
							{/key}
							<span class="text-[11px] text-text-tertiary">
								Overrides the flow-wide default retry. Leave off to inherit it; set max attempts to
								0 to opt out. For a Branch, this retries the operator when it errors (not when it
								returns an unroutable value).
							</span>
						</div>
					{/if}

					<div class="flex flex-col gap-1">
						<span class="text-xs text-text-tertiary">run this node when… (upstream states)</span>
						<select
							class="{FIELD} text-xs"
							value={n.trigger_rule ?? 'all_success'}
							onchange={(e) =>
								updateNode((nd) => ({
									...nd,
									trigger_rule:
										e.currentTarget.value === 'all_success'
											? undefined
											: (e.currentTarget.value as TriggerRule)
								}))}
						>
							<option value="all_success">All upstreams succeeded (default)</option>
							<option value="none_failed_min_one_success">
								None failed, ≥1 succeeded (join after a branch)
							</option>
							<option value="all_done">All upstreams done — success or fail (cleanup)</option>
							<option value="all_failed">All upstreams failed (error handler)</option>
						</select>
						<span class="text-[11px] text-text-tertiary">
							How this node reacts to its upstreams' success/failure.
						</span>
					</div>

					<div class="flex flex-col gap-1">
						<span class="text-xs text-text-tertiary">skip this node when…</span>
						{#key n.id}
							<ConditionBuilder
								value={n.skip_if}
								steps={ancestorIds(n.id)}
								onCommit={(v) => updateNode((nd) => ({ ...nd, skip_if: v }))}
							/>
						{/key}
						<span class="text-[11px] text-text-tertiary">
							Leave empty to always run. When the condition is true, this node is skipped; whether
							its dependents still run is decided by their own trigger rule.
						</span>
					</div>
				{:else if selectedEdge}
					{@const ed = selectedEdge}
					<h3 class="text-xs font-semibold tracking-wide text-text-tertiary uppercase">Edge</h3>
					<p class="text-xs text-text-secondary">{ed.source} → {ed.target}</p>
					{#if selectedEdgeIsBranch}
						<!-- Routed by the branch operator's result, not a `when` expression. -->
						<span class="text-[11px] text-text-tertiary">
							This edge runs when the branch's result matches the case below.
						</span>
						<label class="flex items-center gap-2 text-xs text-text-secondary">
							<input
								type="checkbox"
								checked={selectedEdgeCase?.kind === 'default'}
								onchange={(e) =>
									setEdgeCase(
										e.currentTarget.checked
											? { kind: 'default' }
											: { kind: 'match', value: '' }
									)}
							/>
							Default (else) — runs when no other case matches
						</label>
						{#if selectedEdgeCase?.kind !== 'default'}
							<label class="flex flex-col gap-1">
								<span class="text-xs text-text-tertiary">case value</span>
								{#key ed.id}
									<input
										class="{FIELD} font-mono text-xs"
										placeholder="e.g. paid"
										value={selectedEdgeCase?.kind === 'match'
											? formatCaseValue(selectedEdgeCase.value)
											: ''}
										onchange={(e) =>
											setEdgeCase({ kind: 'match', value: parseCaseValue(e.currentTarget.value) })}
									/>
								{/key}
								<span class="text-[11px] text-text-tertiary">
									Matched against the branch result by value. Bare text is a string; <code
										class="text-text-secondary">42</code
									>/<code class="text-text-secondary">true</code> are parsed as number/boolean.
								</span>
							</label>
						{/if}
					{:else}
						<span class="text-[11px] text-text-tertiary">
							A plain edge is a dependency — the target runs after this node per the target's
							trigger rule. For conditional routing, use a Branch node; to make a node react to
							upstream success/failure, set its trigger rule.
						</span>
					{/if}
					<button class="text-left text-xs text-error hover:underline" onclick={deleteSelectedEdge}>
						Delete edge
					</button>
				{:else}
					<h3 class="text-xs font-semibold tracking-wide text-text-tertiary uppercase">
						Flow settings
					</h3>
					<div class="flex flex-col gap-1">
						<span class="text-xs text-text-tertiary">max concurrent nodes</span>
						<input
							class={FIELD}
							type="number"
							min="1"
							placeholder="unlimited"
							value={maxConcurrent}
							onchange={(e) => editFlowSettings(() => (maxConcurrent = e.currentTarget.value))}
						/>
						<span class="text-[11px] text-text-tertiary">
							How many nodes may run at once within this flow. Empty = unlimited (a Map counts as
							one node).
						</span>
					</div>
					<div class="flex flex-col gap-1">
						<span class="text-xs text-text-tertiary">on error — run a script if the flow fails</span>
						<select
							class={FIELD}
							value={onError?.body.kind === 'script' ? onError.body.script_id : ''}
							onchange={(e) =>
								editFlowSettings(() => {
									const id = e.currentTarget.value;
									onError = id ? { id: 'on_error', body: { kind: 'script', script_id: id } } : null;
								})}
						>
							<option value="">None</option>
							{#each scripts as s (s.script_id)}
								<option value={s.script_id}>{s.name || s.path}</option>
							{/each}
						</select>
						<span class="text-[11px] text-text-tertiary">
							Runs once any node ends up failed (the flow still reports failed).
						</span>
						{#if onError?.body.kind === 'script'}
							{#key onError.body.script_id}
								<InputBindings
									inputs={onError.body.inputs}
									steps={[]}
									onCommit={(v) =>
										editFlowSettings(() => {
											if (onError?.body.kind === 'script') {
												onError = { ...onError, body: { ...onError.body, inputs: v } };
											}
										})}
								/>
							{/key}
							<span class="text-[11px] text-text-tertiary">
								Context available: <code>flow.input.flow_run_id</code> and
								<code>flow.input.failed</code> (list of failed node ids).
							</span>
						{/if}
					</div>
					<div class="flex flex-col gap-1">
						<span class="text-xs text-text-tertiary">default retry (flow-wide)</span>
						{#key flowRetry === null}
							<RetryPolicyEditor
								value={flowRetry ?? undefined}
								onCommit={(retry) => editFlowSettings(() => (flowRetry = retry ?? null))}
							/>
						{/key}
						<span class="text-[11px] text-text-tertiary">
							Applies to every node that doesn't set its own retry. A node overrides this with its
							own policy, or sets max attempts to 0 to opt out.
						</span>
					</div>
					<p class="mt-2 border-t border-border-subtle pt-3 text-xs text-text-tertiary">
						Click a script on the left to add a node. Drag a node's bottom dot onto another node's top
						dot to connect them. Click a node or edge to configure it.
					</p>
				{/if}
			</aside>
		</div>
	{/if}
</PageFrame>

{#if !isNew}
	<FlowTriggersModal
		open={scheduleOpen}
		{flowId}
		flowPath={initialPath}
		canManage={canManageSchedules}
		onClose={() => (scheduleOpen = false)}
	/>
{/if}
