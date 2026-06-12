<script lang="ts">
	import { SvelteFlow, Controls, type Node, type Edge } from '@xyflow/svelte';
	import '@xyflow/svelte/dist/style.css';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';

	import FlowStepNode from '$lib/components/flows/FlowStepNode.svelte';
	import { specToGraph, type ScriptLabel, type StepNodeData } from '$lib/services/flow-graph';
	import type { FlowRunState, FlowSpec, NodeRunState } from '$lib/types';

	interface Props {
		spec: FlowSpec;
		/** Live per-node status overlaid on the graph. */
		flowStatus?: FlowRunState;
		/** Resolve a script_id to a display path (for node titles). */
		scriptLabel?: ScriptLabel;
	}
	let { spec, flowStatus, scriptLabel }: Props = $props();

	const nodeTypes = { step: FlowStepNode };

	// Each node's child run id, for click-through to that run's detail page.
	const runIds = $derived.by(() => {
		const m: Record<string, string> = {};
		for (const s of flowStatus?.nodes ?? []) {
			if ('run_id' in s && s.run_id) m[s.id] = s.run_id;
			// Map nodes have no top-level run_id; drill into the first child run.
			else if ('fanout' in s && s.fanout?.children?.length) m[s.id] = s.fanout.children[0];
		}
		return m;
	});

	function openNode(id: string): void {
		const child = runIds[id];
		if (!child) return;
		// Carry a return URL so the child run's Back comes back to this graph.
		const from = encodeURIComponent(page.url.pathname + page.url.search);
		void goto(`/runs/${child}?from=${from}`);
	}

	// Colour edges by their endpoints' run state, matching the editor overlay:
	// completed path → green, in-progress → accent + animated, failed target → red.
	function colorEdges(es: Edge[], status?: FlowRunState): Edge[] {
		const by = new Map<string, NodeRunState['state']>(
			(status?.nodes ?? []).map((s) => [s.id, s.state])
		);
		return es.map((e) => {
			const src = by.get(e.source);
			const tgt = by.get(e.target);
			let stroke: string | undefined;
			let animated = false;
			if (src === 'succeeded' && tgt === 'succeeded') stroke = 'var(--color-success)';
			else if (src === 'succeeded' && tgt === 'running') {
				stroke = 'var(--color-accent)';
				animated = true;
			} else if (tgt === 'failed') stroke = 'var(--color-error)';
			return { ...e, animated, style: stroke ? `stroke: ${stroke}; stroke-width: 2;` : undefined };
		});
	}

	const graph = $derived.by(() => {
		const g = specToGraph(spec, flowStatus, scriptLabel);
		const nodes = g.nodes.map((n) => ({
			...n,
			data: { ...(n.data as StepNodeData), onSelect: openNode, onDelete: undefined, selected: false }
		}));
		return { nodes, edges: colorEdges(g.edges, flowStatus) };
	});

	// SvelteFlow wants writable node/edge state; mirror the derived graph into it
	// so status updates (from polling) re-render the overlay.
	let nodes = $state.raw<Node[]>([]);
	let edges = $state.raw<Edge[]>([]);
	$effect(() => {
		nodes = graph.nodes;
	});
	$effect(() => {
		edges = graph.edges;
	});
</script>

<div class="h-full min-h-0 w-full">
	<SvelteFlow
		bind:nodes
		bind:edges
		{nodeTypes}
		fitView
		fitViewOptions={{ maxZoom: 1, padding: 0.25 }}
		maxZoom={1.5}
		minZoom={0.4}
		nodesDraggable={false}
		nodesConnectable={false}
		elementsSelectable={false}
		proOptions={{ hideAttribution: true }}
	>
		<Controls showLock={false} />
	</SvelteFlow>
</div>
