// Map between a FlowSpec (DAG of nodes + edges) and an xyflow graph.
//   - specToGraph: spec -> xyflow nodes/edges (for the editor + read-only run view)
//   - graphToSpec: xyflow nodes/edges -> spec (the editor serializes drag/drop edits)
// Node positions come from each node's `ui` field when present, else a dagre
// auto-layout. A FlowRunState can be overlaid so nodes show live status.
import dagre from '@dagrejs/dagre';
import type { Edge, Node } from '@xyflow/svelte';

import type { BranchCase, FlowNode, FlowRunState, FlowSpec, NodeBody, RetryPolicy } from '$lib/types';

export const STEP_NODE_W = 240;
export const STEP_NODE_H = 64;

export type StepStatus = 'idle' | 'pending' | 'running' | 'succeeded' | 'failed' | 'skipped';

/** Data carried by each `step` node; consumed by FlowStepNode.svelte. */
export interface StepNodeData {
	node: FlowNode;
	title: string;
	subtitle: string;
	status: StepStatus;
	/** A script node whose script_id no longer resolves (deleted/moved away).
	 *  Only set when a scriptLabel resolver was supplied, so a missing resolver
	 *  (e.g. the read-only run view) never falsely flags nodes. */
	broken?: boolean;
	selected?: boolean;
	onSelect?: (id: string) => void;
	onDelete?: (id: string) => void;
	[key: string]: unknown;
}

/** Resolve a script_id to a human label (path/name) for display. */
export type ScriptLabel = (scriptId: string) => string | undefined;

/** Card title: prefer a custom summary, else the referenced script's name
 *  (path leaf) so the card reads as the script — the node id stays an internal
 *  reference handle (shown only in the settings panel). Falls back to the id
 *  when there's no summary/script (a broken ref). */
export function nodeTitle(n: FlowNode, scriptLabel?: ScriptLabel): string {
	if (n.summary?.trim()) return n.summary.trim();
	if (n.body.kind === 'script') {
		const path = scriptLabel?.(n.body.script_id);
		if (path) return path.split('/').pop() || path;
	}
	return n.id;
}

export function subtitleFor(body: NodeBody, scriptLabel?: ScriptLabel): string {
	switch (body.kind) {
		case 'script':
			return `script · ${scriptLabel?.(body.script_id) ?? body.script_id}`;
		case 'branch':
			return `branch · ${taskLabel(body.task, scriptLabel)}`;
	}
}

/** Short label for a wrapped task (Branch): the script's name or the kind. */
function taskLabel(task: NodeBody, scriptLabel?: ScriptLabel): string {
	if (task.kind === 'script') {
		const path = scriptLabel?.(task.script_id) ?? task.script_id;
		return path.split('/').pop() || path;
	}
	return task.kind;
}

/** Human label shown on a Branch node's outgoing edge. */
export function branchCaseLabel(c: BranchCase | undefined): string | undefined {
	if (!c) return undefined;
	if (c.kind === 'default') return 'else';
	return `= ${formatCaseValue(c.value)}`;
}

/** Render a case value compactly (unquoted strings, JSON for the rest). */
export function formatCaseValue(value: unknown): string {
	return typeof value === 'string' ? value : JSON.stringify(value);
}

function statusIndex(runState?: FlowRunState): Map<string, StepStatus> {
	const m = new Map<string, StepStatus>();
	if (runState) {
		for (const s of runState.nodes) m.set(s.id, s.state);
		if (runState.on_error) m.set(runState.on_error.id, runState.on_error.state);
	}
	return m;
}

export function specToGraph(
	spec: FlowSpec,
	runState?: FlowRunState,
	scriptLabel?: ScriptLabel
): { nodes: Node[]; edges: Edge[] } {
	const status = statusIndex(runState);

	const nodes: Node[] = spec.nodes.map((n) => ({
		id: n.id,
		type: 'step',
		position: n.ui ?? { x: 0, y: 0 },
		data: {
			node: n,
			title: nodeTitle(n, scriptLabel),
			subtitle: subtitleFor(n.body, scriptLabel),
			status: status.get(n.id) ?? 'idle',
			// Broken = the referenced script_id didn't resolve. Only meaningful when
			// a resolver was passed (the editor); the run view passes none.
			broken:
				n.body.kind === 'script' && !!scriptLabel && scriptLabel(n.body.script_id) === undefined
		} satisfies StepNodeData
	}));

	const branchIds = new Set(
		spec.nodes.filter((n) => n.body.kind === 'branch').map((n) => n.id)
	);
	const edges: Edge[] = spec.edges.map((e, i) => {
		const isBranch = branchIds.has(e.from);
		return {
			id: `e${i}`,
			source: e.from,
			target: e.to,
			// Restore the handles the user drew to, so routing is stable across
			// save/run/reload instead of xyflow re-picking sides each rebuild.
			sourceHandle: e.from_handle,
			targetHandle: e.to_handle,
			// Branch edges show their routing case; plain edges are pure
			// dependencies (no condition label — conditions live on the node's
			// trigger_rule / a Branch node).
			label: isBranch ? branchCaseLabel(e.case) : undefined,
			data: isBranch ? { case: e.case } : undefined,
			type: 'smoothstep',
			animated: status.get(e.to) === 'running'
		};
	});

	// Only auto-layout when positions are missing (freshly authored / no ui).
	if (spec.nodes.some((n) => !n.ui)) layout(nodes, edges);
	return { nodes, edges };
}

export function graphToSpec(
	nodes: Node[],
	edges: Edge[],
	// Flow-level settings the canvas doesn't represent as nodes/edges; threaded
	// through so they survive an edit→save round-trip instead of being dropped.
	meta?: { max_concurrent?: number; on_error?: FlowNode; retry?: RetryPolicy }
): FlowSpec {
	const branchIds = new Set(
		nodes.filter((n) => (n.data as StepNodeData).node.body.kind === 'branch').map((n) => n.id)
	);
	return {
		...(meta?.on_error ? { on_error: meta.on_error } : {}),
		...(meta?.max_concurrent != null ? { max_concurrent: meta.max_concurrent } : {}),
		...(meta?.retry ? { retry: meta.retry } : {}),
		nodes: nodes.map((n) => {
			const data = n.data as StepNodeData;
			return { ...data.node, ui: { x: Math.round(n.position.x), y: Math.round(n.position.y) } };
		}),
		edges: edges.map((e) => {
			// Persist the handles so the drawn routing survives a round-trip.
			const handles = {
				...(e.sourceHandle ? { from_handle: e.sourceHandle } : {}),
				...(e.targetHandle ? { to_handle: e.targetHandle } : {})
			};
			const base = { from: e.source, to: e.target, ...handles };
			// Edges out of a Branch carry a `case`; all other edges are pure
			// dependencies (no `when` — the editor no longer authors edge conditions;
			// node trigger rules + Branch nodes express conditionality).
			if (branchIds.has(e.source)) {
				const c = (e.data as { case?: BranchCase } | undefined)?.case;
				return c ? { ...base, case: c } : base;
			}
			return base;
		})
	};
}

function layout(nodes: Node[], edges: Edge[]): void {
	const g = new dagre.graphlib.Graph();
	g.setGraph({ rankdir: 'TB', nodesep: 40, ranksep: 64, marginx: 16, marginy: 16 });
	g.setDefaultEdgeLabel(() => ({}));
	for (const n of nodes) g.setNode(n.id, { width: STEP_NODE_W, height: STEP_NODE_H });
	for (const e of edges) g.setEdge(e.source, e.target);
	dagre.layout(g);
	for (const n of nodes) {
		const p = g.node(n.id);
		if (p) n.position = { x: p.x - STEP_NODE_W / 2, y: p.y - STEP_NODE_H / 2 };
	}
}
