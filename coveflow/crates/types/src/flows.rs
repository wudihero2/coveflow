use std::collections::HashMap;
use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The full flow definition (a DAG), serialized into `flow.value` / `run.flow_value`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlowSpec {
    pub nodes: Vec<FlowNode>,
    #[serde(default)]
    pub edges: Vec<FlowEdge>,
    /// Optional global error handler, run when a node fails terminally.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_error: Option<Box<FlowNode>>,
    /// Max concurrently-running *nodes* within one flow (None = unlimited; 0 is
    /// rejected). This is NOT a child-run limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrent: Option<u32>,
    /// Flow-level default retry policy. A node uses its own `retry` if set,
    /// otherwise falls back to this; a node sets `max_attempts: 0` to opt out of
    /// the flow default. Applies to dispatched nodes (Script + Branch operator),
    /// not the `on_error` handler.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<RetryPolicy>,
}

/// A DAG node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlowNode {
    pub id: NodeId,
    pub body: NodeBody,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<RetryPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Skip this node (mark Skipped) when the expression is truthy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip_if: Option<Expr>,
    /// Fan-in trigger rule: how this node aggregates its upstreams' terminal
    /// states to decide whether to run. Absent = [`TriggerRule::AllSuccess`].
    #[serde(default, skip_serializing_if = "TriggerRule::is_default")]
    pub trigger_rule: TriggerRule,
    /// Editor canvas position; ignored by the engine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui: Option<NodePos>,
}

/// How a node aggregates its upstream nodes' terminal states (its fan-in trigger
/// rule) to decide whether it runs or is skipped. A node's upstreams are the
/// sources of its incoming edges; each upstream is `succeeded`, `failed`, or
/// `skipped` by the time this is evaluated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerRule {
    /// Run only when every upstream succeeded (the default).
    #[default]
    AllSuccess,
    /// Run when no upstream failed and at least one succeeded (skipped upstreams
    /// are tolerated). The rule for joining back together after a `Branch`.
    NoneFailedMinOneSuccess,
    /// Run once every upstream is terminal, regardless of outcome (cleanup).
    AllDone,
    /// Run only when every upstream failed (a targeted error handler).
    AllFailed,
}

impl TriggerRule {
    /// True for the default (`AllSuccess`), so it can be omitted from JSON.
    pub fn is_default(&self) -> bool {
        matches!(self, TriggerRule::AllSuccess)
    }
}

/// Node identifier (unique within a flow).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeId(pub String);

impl std::fmt::Display for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Canvas coordinates for the editor (not used during execution).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodePos {
    pub x: f64,
    pub y: f64,
}

/// A dependency edge `from -> to`, optionally gated by a condition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlowEdge {
    pub from: NodeId,
    pub to: NodeId,
    /// Conditional edge: only active when this expression is truthy (evaluated
    /// against the source node's result + flow context). Absent = always active.
    /// Mutually exclusive with `case` (an edge is either a normal `when` edge or
    /// a Branch routing edge, never both).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<Expr>,
    /// Branch routing case. Only valid on edges whose source is a `Branch` node:
    /// the edge is active when the branch's returned key(s) match this case (or
    /// it is the `Default` and no `Match` case matched). Absent on all other
    /// edges.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub case: Option<BranchCase>,
    /// Editor-only: which handle (`t`/`l`/`b`/`r`) the edge attaches to on the
    /// source / target node, so routing stays where the user drew it across
    /// save/run/reload. Ignored by the engine (like `FlowNode::ui`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_handle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_handle: Option<String>,
}

/// A `Branch` node's outgoing-edge selector. The branch's `task` runs and
/// returns a key (or array of keys); each `Match` edge whose `value` is in that
/// set activates, the rest skip, and the `Default` edge activates only when no
/// `Match` matched.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BranchCase {
    /// Active when the branch result set contains `value` (JSON equality).
    Match { value: serde_json::Value },
    /// Active only when no sibling `Match` case matched.
    Default,
}

/// What a node does. `kind` is the serde tag.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NodeBody {
    /// Reference an existing saved script by its stable id (path is a movable
    /// label, so referencing by id survives moves/renames). `hash` optionally
    /// pins a specific version; otherwise the latest version is resolved.
    Script {
        script_id: Uuid,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hash: Option<String>,
        #[serde(default, skip_serializing_if = "HashMap::is_empty")]
        inputs: HashMap<String, InputBinding>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        queue: Option<String>,
    },
    /// Conditional routing node. Runs `task` (a Script) once; its return
    /// value is a key (or array of keys) matched against the outgoing edges'
    /// `BranchCase`. Matching edges activate and the rest are skipped. The result
    /// must be a scalar (string/number/bool) or an array of scalars; otherwise
    /// the node fails.
    Branch { task: Box<NodeBody> },
}

/// Where a node input comes from. `kind` is the serde tag.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InputBinding {
    Static { value: serde_json::Value },
    Expr { expr: Expr },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Expr(pub String);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub backoff: Backoff,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Backoff {
    Fixed {
        delay_ms: u64,
    },
    Exponential {
        base_ms: u64,
        factor: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        jitter: Option<f64>,
    },
}

/// A validation problem found by [`FlowSpec::validate`].
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum FlowError {
    #[error("flow has no nodes")]
    Empty,
    #[error("duplicate node id '{0}'")]
    DuplicateNodeId(String),
    #[error("edge {edge} references unknown node '{node}'")]
    EdgeUnknownNode { edge: String, node: String },
    #[error("node '{0}' has an edge to itself")]
    SelfEdge(String),
    #[error("flow graph contains a cycle")]
    Cycle,
    #[error("max_concurrent must be > 0 (omit it for unlimited)")]
    ZeroMaxConcurrent,
    /// A Branch node's `task` is not a Script body.
    #[error("branch node '{0}' task must be a script body")]
    BranchTaskNotLeaf(String),
    /// A Branch node has no outgoing edges (it routes to nothing).
    #[error("branch node '{0}' has no outgoing edges")]
    BranchNoTargets(String),
    /// An edge out of a Branch node is missing its `case`.
    #[error("edge {0} out of a branch node must have a case")]
    BranchEdgeMissingCase(String),
    /// An edge out of a Branch node also carries a `when` (mutually exclusive).
    #[error("edge {0} out of a branch node must not have a `when`")]
    BranchEdgeHasWhen(String),
    /// An edge carries a `case` but its source is not a Branch node.
    #[error("edge {0} has a case but its source is not a branch node")]
    CaseOnNonBranchEdge(String),
    /// Two outgoing edges of one Branch share the same `Match` value.
    #[error("branch node '{node}' has duplicate case value {value}")]
    DuplicateBranchCase { node: String, value: String },
    /// A Branch node has more than one `Default` outgoing edge.
    #[error("branch node '{0}' has more than one default edge")]
    MultipleBranchDefaults(String),
}

impl FlowSpec {
    /// Validate the DAG: non-empty, unique node ids, edges reference known nodes,
    /// no self-edges, acyclic, and branch tasks are script bodies. Reads as a
    /// checklist — each check lives in its own helper below.
    pub fn validate(&self) -> Result<(), Vec<FlowError>> {
        let mut errors = Vec::new();

        self.check_nonempty(&mut errors);
        self.check_max_concurrent(&mut errors);

        let ids = self.collect_node_ids(&mut errors);
        let branch_ids = self.collect_branch_ids(&mut errors);
        self.check_on_error_handler(&mut errors);

        let branch_edges = self.check_edges(&ids, &branch_ids, &mut errors);
        check_branch_routing(&branch_ids, &branch_edges, &mut errors);

        // Cycles only matter once the graph is structurally sound (every edge
        // references a known node), so gate the check on no prior errors.
        if errors.is_empty() && self.has_cycle() {
            errors.push(FlowError::Cycle);
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    fn check_nonempty(&self, errors: &mut Vec<FlowError>) {
        if self.nodes.is_empty() {
            errors.push(FlowError::Empty);
        }
    }

    fn check_max_concurrent(&self, errors: &mut Vec<FlowError>) {
        // max_concurrent == 0 would make the running-count check always true, so
        // no node ever dispatches and the flow suspends forever. Reject it;
        // "unlimited" is expressed by omitting the field (None).
        if self.max_concurrent == Some(0) {
            errors.push(FlowError::ZeroMaxConcurrent);
        }
    }

    /// Gather every node id, reporting any duplicates. The returned set is the
    /// known-node universe used to validate edge endpoints.
    fn collect_node_ids(&self, errors: &mut Vec<FlowError>) -> HashSet<&str> {
        let mut ids = HashSet::new();
        for node in &self.nodes {
            if !ids.insert(node.id.0.as_str()) {
                errors.push(FlowError::DuplicateNodeId(node.id.0.clone()));
            }
        }
        ids
    }

    /// Gather Branch node ids, reporting any whose `task` is not a Script body.
    fn collect_branch_ids(&self, errors: &mut Vec<FlowError>) -> HashSet<&str> {
        let mut branch_ids = HashSet::new();
        for node in &self.nodes {
            if let NodeBody::Branch { task } = &node.body {
                branch_ids.insert(node.id.0.as_str());
                if !matches!(task.as_ref(), NodeBody::Script { .. }) {
                    errors.push(FlowError::BranchTaskNotLeaf(node.id.0.clone()));
                }
            }
        }
        branch_ids
    }

    /// The on_error handler can't be a Branch: handlers have no outgoing edges,
    /// so a branch could never route anywhere.
    fn check_on_error_handler(&self, errors: &mut Vec<FlowError>) {
        if let Some(h) = &self.on_error {
            if matches!(&h.body, NodeBody::Branch { .. }) {
                errors.push(FlowError::BranchNoTargets(h.id.0.clone()));
            }
        }
    }

    /// Validate every edge (endpoints exist, no self-edges, branch routing rules)
    /// and return per-branch routing tallies for [`check_branch_routing`].
    fn check_edges<'a>(
        &'a self,
        ids: &HashSet<&str>,
        branch_ids: &HashSet<&str>,
        errors: &mut Vec<FlowError>,
    ) -> HashMap<&'a str, BranchRouting> {
        let mut branch_edges: HashMap<&str, BranchRouting> = HashMap::new();
        for edge in &self.edges {
            let label = format!("{}->{}", edge.from, edge.to);
            if edge.from == edge.to {
                errors.push(FlowError::SelfEdge(edge.from.0.clone()));
            }
            for endpoint in [&edge.from, &edge.to] {
                if !ids.contains(endpoint.0.as_str()) {
                    errors.push(FlowError::EdgeUnknownNode {
                        edge: label.clone(),
                        node: endpoint.0.clone(),
                    });
                }
            }

            // Edges out of a Branch route by `case`; all other edges must not.
            if branch_ids.contains(edge.from.0.as_str()) {
                check_branch_edge(edge, &label, &mut branch_edges, errors);
            } else if edge.case.is_some() {
                errors.push(FlowError::CaseOnNonBranchEdge(label));
            }
        }
        branch_edges
    }

    /// Kahn's algorithm: a DAG topo-sorts iff every node is removable.
    fn has_cycle(&self) -> bool {
        let mut indeg: HashMap<&str, usize> =
            self.nodes.iter().map(|n| (n.id.0.as_str(), 0)).collect();
        let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
        for e in &self.edges {
            *indeg.entry(e.to.0.as_str()).or_insert(0) += 1;
            adj.entry(e.from.0.as_str())
                .or_default()
                .push(e.to.0.as_str());
        }
        let mut queue: Vec<&str> = indeg
            .iter()
            .filter(|(_, d)| **d == 0)
            .map(|(&n, _)| n)
            .collect();
        let mut visited = 0;
        while let Some(n) = queue.pop() {
            visited += 1;
            if let Some(outs) = adj.get(n) {
                for &m in outs {
                    if let Some(d) = indeg.get_mut(m) {
                        *d -= 1;
                        if *d == 0 {
                            queue.push(m);
                        }
                    }
                }
            }
        }
        visited != self.nodes.len()
    }

    /// Node ids with no incoming edge — the roots that start the flow.
    pub fn roots(&self) -> Vec<&NodeId> {
        let with_incoming: HashSet<&str> = self.edges.iter().map(|e| e.to.0.as_str()).collect();
        self.nodes
            .iter()
            .map(|n| &n.id)
            .filter(|id| !with_incoming.contains(id.0.as_str()))
            .collect()
    }

    /// Incoming edges of a node.
    pub fn incoming(&self, id: &NodeId) -> Vec<&FlowEdge> {
        self.edges.iter().filter(|e| &e.to == id).collect()
    }
}

/// Routing tally for one Branch node, accumulated while validating its edges.
#[derive(Default)]
struct BranchRouting {
    /// Total outgoing edges.
    outgoing: usize,
    /// Edges marked `Default`.
    defaults: usize,
    /// Dedup keys of `Match` case values seen so far.
    match_values: HashSet<String>,
}

/// Validate one edge leaving a Branch node and fold it into the branch's tally.
fn check_branch_edge<'a>(
    edge: &'a FlowEdge,
    label: &str,
    branch_edges: &mut HashMap<&'a str, BranchRouting>,
    errors: &mut Vec<FlowError>,
) {
    let acc = branch_edges.entry(edge.from.0.as_str()).or_default();
    acc.outgoing += 1;
    match &edge.case {
        None => errors.push(FlowError::BranchEdgeMissingCase(label.to_string())),
        Some(BranchCase::Default) => acc.defaults += 1,
        Some(BranchCase::Match { value }) => {
            if !acc.match_values.insert(branch_case_key(value)) {
                errors.push(FlowError::DuplicateBranchCase {
                    node: edge.from.0.clone(),
                    value: value.to_string(),
                });
            }
        }
    }
    // `case` and `when` are mutually exclusive on a branch edge.
    if edge.when.is_some() {
        errors.push(FlowError::BranchEdgeHasWhen(label.to_string()));
    }
}

/// Stable dedup key for a branch `Match` value. Numbers normalize to f64 so `1`
/// and `1.0` collide — matching the engine's numeric routing equality (otherwise
/// both would match the same result at runtime).
fn branch_case_key(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Number(n) => n
            .as_f64()
            .map_or_else(|| value.to_string(), |f| format!("#num:{f}")),
        _ => value.to_string(),
    }
}

/// Every branch must route somewhere and have at most one default edge.
fn check_branch_routing(
    branch_ids: &HashSet<&str>,
    branch_edges: &HashMap<&str, BranchRouting>,
    errors: &mut Vec<FlowError>,
) {
    for id in branch_ids {
        let routing = branch_edges.get(id);
        if routing.map_or(0, |r| r.outgoing) == 0 {
            errors.push(FlowError::BranchNoTargets((*id).to_string()));
        }
        if routing.map_or(0, |r| r.defaults) > 1 {
            errors.push(FlowError::MultipleBranchDefaults((*id).to_string()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigger_rule_round_trips_and_defaults() {
        // Default (all_success) is omitted from JSON; an explicit rule survives.
        let mut n = script("a");
        assert!(!serde_json::to_string(&n).unwrap().contains("trigger_rule"));
        n.trigger_rule = TriggerRule::AllFailed;
        let json = serde_json::to_string(&n).unwrap();
        assert!(json.contains(r#""trigger_rule":"all_failed""#));
        let back: FlowNode = serde_json::from_str(&json).unwrap();
        assert_eq!(back.trigger_rule, TriggerRule::AllFailed);
        // A flow authored before trigger_rule existed defaults to all_success.
        let legacy: FlowNode = serde_json::from_str(
            r#"{"id":"a","body":{"kind":"script","script_id":"00000000-0000-0000-0000-000000000000"}}"#,
        )
        .unwrap();
        assert_eq!(legacy.trigger_rule, TriggerRule::AllSuccess);
    }

    fn script(id: &str) -> FlowNode {
        FlowNode {
            id: NodeId(id.into()),
            body: NodeBody::Script {
                script_id: Uuid::nil(),
                hash: None,
                inputs: HashMap::new(),
                queue: None,
            },
            retry: None,
            summary: None,
            skip_if: None,
            trigger_rule: Default::default(),
            ui: None,
        }
    }

    fn edge(from: &str, to: &str) -> FlowEdge {
        FlowEdge {
            from: NodeId(from.into()),
            to: NodeId(to.into()),
            when: None,
            case: None,
            from_handle: None,
            to_handle: None,
        }
    }

    #[test]
    fn round_trips_dag() {
        let spec = FlowSpec {
            nodes: vec![script("a"), script("b"), script("c")],
            edges: vec![edge("a", "b"), edge("a", "c")],
            on_error: None,
            max_concurrent: Some(2),
            retry: None,
        };
        let json = serde_json::to_string(&spec).unwrap();
        let back: FlowSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(spec, back);
        spec.validate().expect("valid diamond-ish dag");
    }

    #[test]
    fn roots_and_incoming() {
        let spec = FlowSpec {
            nodes: vec![script("a"), script("b"), script("c"), script("d")],
            edges: vec![
                edge("a", "b"),
                edge("a", "c"),
                edge("b", "d"),
                edge("c", "d"),
            ],
            on_error: None,
            max_concurrent: None,
            retry: None,
        };
        assert_eq!(
            spec.roots()
                .iter()
                .map(|n| n.0.as_str())
                .collect::<Vec<_>>(),
            vec!["a"]
        );
        assert_eq!(spec.incoming(&NodeId("d".into())).len(), 2);
    }

    #[test]
    fn detects_cycle() {
        let spec = FlowSpec {
            nodes: vec![script("a"), script("b")],
            edges: vec![edge("a", "b"), edge("b", "a")],
            on_error: None,
            max_concurrent: None,
            retry: None,
        };
        assert!(spec.validate().unwrap_err().contains(&FlowError::Cycle));
    }

    #[test]
    fn rejects_zero_max_concurrent() {
        let spec = FlowSpec {
            nodes: vec![script("a")],
            edges: vec![],
            on_error: None,
            max_concurrent: Some(0),
            retry: None,
        };
        assert!(
            spec.validate()
                .unwrap_err()
                .contains(&FlowError::ZeroMaxConcurrent)
        );
    }

    #[test]
    fn rejects_unknown_and_self_edges() {
        let spec = FlowSpec {
            nodes: vec![script("a")],
            edges: vec![edge("a", "a"), edge("a", "ghost")],
            on_error: None,
            max_concurrent: None,
            retry: None,
        };
        let errs = spec.validate().unwrap_err();
        assert!(errs.iter().any(|e| matches!(e, FlowError::SelfEdge(_))));
        assert!(
            errs.iter()
                .any(|e| matches!(e, FlowError::EdgeUnknownNode { .. }))
        );
    }

    // --- Branch ----------------------------------------------------------

    fn branch(id: &str) -> FlowNode {
        FlowNode {
            id: NodeId(id.into()),
            body: NodeBody::Branch {
                task: Box::new(NodeBody::Script {
                    script_id: Uuid::nil(),
                    hash: None,
                    inputs: HashMap::new(),
                    queue: None,
                }),
            },
            retry: None,
            summary: None,
            skip_if: None,
            trigger_rule: Default::default(),
            ui: None,
        }
    }

    fn match_edge(from: &str, to: &str, value: serde_json::Value) -> FlowEdge {
        FlowEdge {
            from: NodeId(from.into()),
            to: NodeId(to.into()),
            when: None,
            case: Some(BranchCase::Match { value }),
            from_handle: None,
            to_handle: None,
        }
    }

    fn default_edge(from: &str, to: &str) -> FlowEdge {
        FlowEdge {
            from: NodeId(from.into()),
            to: NodeId(to.into()),
            when: None,
            case: Some(BranchCase::Default),
            from_handle: None,
            to_handle: None,
        }
    }

    #[test]
    fn branch_node_round_trips_and_validates() {
        let spec = FlowSpec {
            nodes: vec![
                branch("b"),
                script("paid"),
                script("failed"),
                script("other"),
            ],
            edges: vec![
                match_edge("b", "paid", "paid".into()),
                match_edge("b", "failed", "failed".into()),
                default_edge("b", "other"),
            ],
            on_error: None,
            max_concurrent: None,
            retry: None,
        };
        let back: FlowSpec = serde_json::from_str(&serde_json::to_string(&spec).unwrap()).unwrap();
        assert_eq!(spec, back);
        spec.validate()
            .expect("switch branch with default is valid");
    }

    #[test]
    fn branch_task_must_be_leaf() {
        let mut spec = FlowSpec {
            nodes: vec![branch("b"), script("x")],
            edges: vec![match_edge("b", "x", "go".into())],
            on_error: None,
            max_concurrent: None,
            retry: None,
        };
        // A branch task must be a Script; a nested Branch is not a valid task.
        spec.nodes[0].body = NodeBody::Branch {
            task: Box::new(NodeBody::Branch {
                task: Box::new(NodeBody::Script {
                    script_id: Uuid::nil(),
                    hash: None,
                    inputs: HashMap::new(),
                    queue: None,
                }),
            }),
        };
        assert!(
            spec.validate()
                .unwrap_err()
                .iter()
                .any(|e| matches!(e, FlowError::BranchTaskNotLeaf(_)))
        );
    }

    #[test]
    fn branch_out_edge_must_have_case() {
        let spec = FlowSpec {
            nodes: vec![branch("b"), script("x")],
            edges: vec![edge("b", "x")], // no case
            on_error: None,
            max_concurrent: None,
            retry: None,
        };
        assert!(
            spec.validate()
                .unwrap_err()
                .iter()
                .any(|e| matches!(e, FlowError::BranchEdgeMissingCase(_)))
        );
    }

    #[test]
    fn case_on_non_branch_edge_rejected() {
        let spec = FlowSpec {
            nodes: vec![script("a"), script("b")],
            edges: vec![match_edge("a", "b", "x".into())],
            on_error: None,
            max_concurrent: None,
            retry: None,
        };
        assert!(
            spec.validate()
                .unwrap_err()
                .iter()
                .any(|e| matches!(e, FlowError::CaseOnNonBranchEdge(_)))
        );
    }

    #[test]
    fn branch_rejects_duplicate_case_and_multiple_defaults() {
        let spec = FlowSpec {
            nodes: vec![
                branch("b"),
                script("x"),
                script("y"),
                script("z"),
                script("w"),
            ],
            edges: vec![
                match_edge("b", "x", "dup".into()),
                match_edge("b", "y", "dup".into()),
                default_edge("b", "z"),
                default_edge("b", "w"),
            ],
            on_error: None,
            max_concurrent: None,
            retry: None,
        };
        let errs = spec.validate().unwrap_err();
        assert!(
            errs.iter()
                .any(|e| matches!(e, FlowError::DuplicateBranchCase { .. }))
        );
        assert!(
            errs.iter()
                .any(|e| matches!(e, FlowError::MultipleBranchDefaults(_)))
        );
    }

    #[test]
    fn branch_duplicate_case_normalizes_numbers() {
        // case 1 and 1.0 must collide (engine routes them equally).
        let spec = FlowSpec {
            nodes: vec![branch("b"), script("x"), script("y")],
            edges: vec![
                match_edge("b", "x", 1.into()),
                match_edge("b", "y", (1.0).into()),
            ],
            on_error: None,
            max_concurrent: None,
            retry: None,
        };
        assert!(
            spec.validate()
                .unwrap_err()
                .iter()
                .any(|e| matches!(e, FlowError::DuplicateBranchCase { .. }))
        );
    }

    #[test]
    fn branch_with_no_targets_rejected() {
        let spec = FlowSpec {
            nodes: vec![branch("b")],
            edges: vec![],
            on_error: None,
            max_concurrent: None,
            retry: None,
        };
        assert!(
            spec.validate()
                .unwrap_err()
                .iter()
                .any(|e| matches!(e, FlowError::BranchNoTargets(_)))
        );
    }

    #[test]
    fn branch_as_on_error_handler_rejected() {
        // A handler has no outgoing edges, so a Branch can never route there.
        let spec = FlowSpec {
            nodes: vec![script("a")],
            edges: vec![],
            on_error: Some(Box::new(branch("h"))),
            max_concurrent: None,
            retry: None,
        };
        assert!(
            spec.validate()
                .unwrap_err()
                .iter()
                .any(|e| matches!(e, FlowError::BranchNoTargets(_)))
        );
    }

    #[test]
    fn branch_edge_with_when_rejected() {
        let spec = FlowSpec {
            nodes: vec![branch("b"), script("x")],
            edges: vec![FlowEdge {
                from: NodeId("b".into()),
                to: NodeId("x".into()),
                when: Some(Expr("true".into())),
                case: Some(BranchCase::Match { value: "go".into() }),
                from_handle: None,
                to_handle: None,
            }],
            on_error: None,
            max_concurrent: None,
            retry: None,
        };
        assert!(
            spec.validate()
                .unwrap_err()
                .iter()
                .any(|e| matches!(e, FlowError::BranchEdgeHasWhen(_)))
        );
    }
}
