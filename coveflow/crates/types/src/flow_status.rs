//! Flow runtime state (DAG model).
//!
//! `FlowRunState` records the state of every node of a [`crate::flows::FlowSpec`]
//! DAG, serialized into `run_flow_status.flow_status` (JSONB). There is no linear
//! cursor: a node becomes runnable once its incoming edges are satisfied, so the
//! scheduler keys everything off per-node state. Lifecycle names follow the
//! industry-common vocabulary (pending → running → succeeded/failed/skipped).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::flows::NodeId;

/// Execution state of one flow run's DAG.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlowRunState {
    pub nodes: Vec<NodeState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_error: Option<NodeState>,
    /// Retry attempts so far, keyed by node id.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub retries: HashMap<String, u32>,
}

/// State of a single node. `state` is the serde tag.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum NodeState {
    /// Not started (incoming edges not yet satisfied).
    Pending { id: NodeId },
    /// Executing. `run_id` is the child run executing this node.
    Running {
        id: NodeId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run_id: Option<Uuid>,
    },
    Succeeded {
        id: NodeId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run_id: Option<Uuid>,
        result: serde_json::Value,
    },
    Failed {
        id: NodeId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run_id: Option<Uuid>,
        error: serde_json::Value,
    },
    /// Skipped: a node's incoming edges were all inactive (untaken condition),
    /// an upstream failed, or its own `skip_if` was truthy.
    Skipped { id: NodeId },
}

impl NodeState {
    pub fn id(&self) -> &NodeId {
        match self {
            Self::Pending { id }
            | Self::Running { id, .. }
            | Self::Succeeded { id, .. }
            | Self::Failed { id, .. }
            | Self::Skipped { id } => id,
        }
    }

    pub fn run_id(&self) -> Option<Uuid> {
        match self {
            Self::Running { run_id, .. }
            | Self::Succeeded { run_id, .. }
            | Self::Failed { run_id, .. } => *run_id,
            Self::Pending { .. } | Self::Skipped { .. } => None,
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Succeeded { .. } | Self::Failed { .. } | Self::Skipped { .. }
        )
    }

    pub fn is_success(&self) -> bool {
        matches!(self, Self::Succeeded { .. })
    }
}

impl FlowRunState {
    /// Initialize every node as Pending.
    pub fn init(node_ids: impl IntoIterator<Item = NodeId>) -> Self {
        Self {
            nodes: node_ids
                .into_iter()
                .map(|id| NodeState::Pending { id })
                .collect(),
            on_error: None,
            retries: HashMap::new(),
        }
    }

    pub fn get(&self, id: &NodeId) -> Option<&NodeState> {
        self.nodes.iter().find(|s| s.id() == id)
    }

    pub fn set(&mut self, state: NodeState) {
        if let Some(slot) = self.nodes.iter_mut().find(|s| s.id() == state.id()) {
            *slot = state;
        } else {
            self.nodes.push(state);
        }
    }

    /// Every succeeded node's result as `{ <id>: { "result": <value> } }`. This
    /// is the shared shape behind both the flow expression `steps.*` namespace
    /// and the injected run context's `ctx["steps"]`.
    pub fn succeeded_steps(&self) -> serde_json::Map<String, serde_json::Value> {
        let mut steps = serde_json::Map::new();
        for s in &self.nodes {
            if let NodeState::Succeeded { id, result, .. } = s {
                steps.insert(id.0.clone(), serde_json::json!({ "result": result }));
            }
        }
        steps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_dag_state() {
        let mut state = FlowRunState::init([NodeId("a".into()), NodeId("b".into())]);
        state.set(NodeState::Succeeded {
            id: NodeId("a".into()),
            run_id: Some(Uuid::nil()),
            result: serde_json::json!({"total": 41}),
        });
        state.retries.insert("b".into(), 1);

        let json = serde_json::to_string(&state).unwrap();
        let back: FlowRunState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, back);

        assert!(state.get(&NodeId("a".into())).unwrap().is_success());
        assert!(matches!(
            state.get(&NodeId("b".into())),
            Some(NodeState::Pending { .. })
        ));
    }

    #[test]
    fn set_overwrites_in_place() {
        let mut state = FlowRunState::init([NodeId("a".into())]);
        state.set(NodeState::Running {
            id: NodeId("a".into()),
            run_id: Some(Uuid::nil()),
        });
        assert_eq!(state.nodes.len(), 1);
        assert!(!state.nodes[0].is_terminal());
    }
}
