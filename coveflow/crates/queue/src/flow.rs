//! Flow execution engine (dependency-based DAG scheduler).
//!
//! A flow is a queue run (`kind = 'flow'`). A worker claims it and calls
//! [`advance_flow`], which:
//!   1. reconciles every Running node against its child run(s) completion,
//!   2. propagates skips and dispatches every node whose incoming edges are now
//!      satisfied (fan-out — multiple nodes can start at once),
//!   3. suspends the flow run (parks `scheduled_for`) when waiting on children,
//!      or finishes it when the whole DAG is terminal.
//!
//! When a child finishes, [`crate::finish_run`] calls [`on_child_complete`],
//! which wakes the parent flow (serialized via `run_flow_status FOR UPDATE`, so
//! the wake can never be clobbered by a concurrent suspend).
//!
//! Edge semantics: an edge `from->to` is *active* when `from` succeeded and the
//! edge's optional `when` is truthy (Branch edges activate by case-matching).
//! Each node then aggregates its incoming edges' effective states
//! (success / failed / inactive) per its `trigger_rule`: `all_success` default,
//! `none_failed_min_one_success` for joining after a Branch, `all_done` for
//! cleanup, `all_failed` for error handlers; `skip_if` is a final veto. Roots
//! (no incoming edges) run immediately. A failure does not abort the run — the
//! DAG runs to a standstill so handler/cleanup nodes can run, then the flow
//! fails (after `on_error`) iff any node ended Failed.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use coveflow_types::flow_status::{FlowRunState, NodeState};
use coveflow_types::flows::{
    Backoff, BranchCase, Expr, FlowEdge, FlowNode, FlowSpec, InputBinding, NodeBody, NodeId,
    TriggerRule,
};
use serde_json::{Map, Value};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::{QueueError, QueueResult};

/// Wrap a node-level runtime error as a `NodeState::Failed` error payload.
fn node_error_value(e: &QueueError) -> Value {
    serde_json::json!({ "error": { "message": e.to_string() } })
}

/// Result of one [`advance_flow`] pass.
#[derive(Debug, Clone, PartialEq)]
pub enum FlowProgress {
    Suspended,
    Completed { result: Value },
    Failed { error: Value },
}

struct FlowRun {
    id: Uuid,
    workspace_id: String,
    tag: String,
    created_by: String,
    root: Uuid,
    input: Value,
    spec: FlowSpec,
    // Fields backing the expression `run.*` namespace (built once per load, no
    // extra query — the flow run row already carries them).
    created_at: DateTime<Utc>,
    flow_path: Option<String>,
    scheduled_time: Option<DateTime<Utc>>,
    data_interval_end: Option<DateTime<Utc>>,
    schedule_id: Option<Uuid>,
    schedule_name: Option<String>,
    schedule_tz: Option<String>,
    trigger_context: Option<Value>,
}

impl FlowRun {
    /// The `run.*` expression namespace: an Airflow-style execution context built
    /// from this flow run's own row (so no extra query) plus the live step
    /// results. Mirrors `build_run_context`, which derives the same shape from
    /// the DB for the worker's `ctx` injection and `get_run`.
    fn run_context_value(&self, state: &FlowRunState) -> Value {
        let ctx = crate::build_from_parts(crate::ContextParts {
            run_id: self.id,
            flow_run_id: Some(self.id),
            flow_path: self.flow_path.clone(),
            created_by: self.created_by.clone(),
            created_at: self.created_at,
            logical_date: self.scheduled_time,
            interval_end: self.data_interval_end,
            schedule_id: self.schedule_id,
            schedule_name: self.schedule_name.clone(),
            schedule_tz: self.schedule_tz.clone(),
            flow_input: Some(self.input.clone()),
            steps: Value::Object(state.succeeded_steps()),
            trigger: self.trigger_context.clone(),
        });
        serde_json::to_value(ctx).unwrap_or(Value::Null)
    }
}

/// Advance a claimed flow run as far as possible in one transaction.
#[tracing::instrument(name = "queue::advance_flow", skip(db), fields(%flow_run_id))]
pub async fn advance_flow(db: &PgPool, flow_run_id: Uuid) -> QueueResult<FlowProgress> {
    let mut tx = db.begin().await?;

    let run = sqlx::query!(
        r#"SELECT r.workspace_id, r.tag, r.created_by, r.created_at, r.root_run,
                  r.args, r.flow_value, r.script_path, r.scheduled_time,
                  r.data_interval_end, r.schedule_id, r.trigger_context,
                  s.name AS "schedule_name?", s.timezone AS "schedule_tz?"
           FROM run r
           LEFT JOIN schedule s ON s.id = r.schedule_id
           WHERE r.id = $1"#,
        flow_run_id
    )
    .fetch_one(&mut *tx)
    .await?;

    let spec: FlowSpec = serde_json::from_value(
        run.flow_value
            .ok_or_else(|| QueueError::Other("flow run missing flow_value".into()))?,
    )
    .map_err(|e| QueueError::Other(format!("invalid flow_value: {e}")))?;

    let flow = FlowRun {
        id: flow_run_id,
        workspace_id: run.workspace_id,
        tag: run.tag,
        created_by: run.created_by,
        root: run.root_run.unwrap_or(flow_run_id),
        input: run.args.unwrap_or(Value::Null),
        spec,
        created_at: run.created_at,
        flow_path: run.script_path,
        scheduled_time: run.scheduled_time,
        data_interval_end: run.data_interval_end,
        schedule_id: run.schedule_id,
        schedule_name: run.schedule_name,
        schedule_tz: run.schedule_tz,
        trigger_context: run.trigger_context,
    };

    let mut state = load_or_init_state(&mut tx, flow_run_id, &flow.spec).await?;
    let progress = schedule(&mut tx, &flow, &mut state).await?;
    save_state(&mut tx, flow_run_id, &state).await?;

    if progress == FlowProgress::Suspended {
        sqlx::query!(
            "UPDATE run_queue
             SET running = FALSE, worker = NULL, started_at = NULL, scheduled_for = 'infinity'
             WHERE id = $1",
            flow_run_id
        )
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(progress)
}

/// One scheduling pass: reconcile running nodes, then dispatch newly-ready ones.
async fn schedule(
    tx: &mut Transaction<'_, Postgres>,
    flow: &FlowRun,
    state: &mut FlowRunState,
) -> QueueResult<FlowProgress> {
    // 1. Reconcile every Running node against its child run. A node-level error
    //    here fails that node rather than aborting the pass; only infra (DB)
    //    errors abort.
    let running_ids: Vec<NodeId> = state
        .nodes
        .iter()
        .filter(|s| matches!(s, NodeState::Running { .. }))
        .map(|s| s.id().clone())
        .collect();
    for id in running_ids {
        if let Err(e) = reconcile_running(tx, flow, state, &id).await {
            if matches!(e, QueueError::Db(_)) {
                return Err(e);
            }
            let run_id = state.get(&id).and_then(|s| s.run_id());
            state.set(NodeState::Failed {
                id: id.clone(),
                run_id,
                error: node_error_value(&e),
            });
        }
    }
    // Reconcile the on_error handler if it is running.
    if let Some(NodeState::Running { .. }) = &state.on_error {
        if let Some(handler) = &flow.spec.on_error {
            let done = reconcile_handler(tx, state, handler).await?;
            if done {
                // Handler finished → the flow fails (handler has run).
                skip_residual_pending(state);
                return Ok(FlowProgress::Failed {
                    error: first_failure(state),
                });
            }
            return Ok(FlowProgress::Suspended);
        }
    }

    // 2. Propagate skips + dispatch ready nodes to a fixed point. A failure does
    //    not stop other work: each node's trigger_rule decides whether it runs
    //    given upstream outcomes, so error-handler / cleanup nodes still
    //    dispatch and downstream all_success nodes are persisted as Skipped. A
    //    node's runtime error (bad expression, missing script, …)
    //    marks that node Failed and scheduling continues; only DB errors abort,
    //    so the skip/handler state is always persisted instead of rolled back.
    let mut running = state
        .nodes
        .iter()
        .filter(|s| matches!(s, NodeState::Running { .. }))
        .count();
    let limit = flow.spec.max_concurrent.map(|m| m as usize);

    // Iterate to a fixed point: skipping/dispatching a node can make others ready.
    loop {
        let mut progressed = false;
        for node in &flow.spec.nodes {
            if !matches!(state.get(&node.id), Some(NodeState::Pending { .. })) {
                continue;
            }
            let ready = match readiness(flow, state, node) {
                Ok(r) => r,
                Err(QueueError::Db(e)) => return Err(QueueError::Db(e)),
                Err(e) => {
                    state.set(NodeState::Failed {
                        id: node.id.clone(),
                        run_id: None,
                        error: node_error_value(&e),
                    });
                    progressed = true;
                    continue;
                }
            };
            match ready {
                Readiness::Wait => {}
                Readiness::Skip => {
                    state.set(NodeState::Skipped {
                        id: node.id.clone(),
                    });
                    progressed = true;
                }
                Readiness::Ready => {
                    // No fail-fast: a node's trigger_rule already decided it should
                    // run given upstream outcomes, so error-handler (`all_failed`)
                    // and cleanup (`all_done`) nodes must still dispatch even after a
                    // failure. The flow's terminal verdict is decided in step 3.
                    if limit.is_some_and(|l| running >= l) {
                        continue;
                    }
                    match dispatch(tx, flow, state, node).await {
                        Ok(()) => {}
                        Err(QueueError::Db(e)) => return Err(QueueError::Db(e)),
                        Err(e) => {
                            state.set(NodeState::Failed {
                                id: node.id.clone(),
                                run_id: None,
                                error: node_error_value(&e),
                            });
                            progressed = true;
                            continue;
                        }
                    }
                    // A successful dispatch always parks the node as Running (a
                    // dispatch error is handled above and marks it Failed).
                    if matches!(state.get(&node.id), Some(NodeState::Running { .. })) {
                        running += 1;
                    }
                    progressed = true;
                }
            }
        }
        if !progressed {
            break;
        }
    }

    // 3. Decide overall progress. We let the DAG run to a standstill before
    //    finalizing so error-handler / cleanup nodes (all_failed / all_done) get
    //    to run: while any node is still Running (or Pending waiting on it) the
    //    flow suspends and is woken when a child completes. Only once every node
    //    is terminal do we finalize — failing the flow (after the on_error
    //    handler) if any node failed, otherwise completing.
    if state.nodes.iter().all(|s| s.is_terminal()) {
        if state
            .nodes
            .iter()
            .any(|s| matches!(s, NodeState::Failed { .. }))
        {
            return fail_flow(tx, flow, state).await;
        }
        Ok(FlowProgress::Completed {
            result: flow_result(flow, state),
        })
    } else {
        Ok(FlowProgress::Suspended)
    }
}

enum Readiness {
    Wait,
    Ready,
    Skip,
}

/// Classify a Pending node: ready to dispatch, should be skipped, or still
/// waiting on upstreams. The node's `trigger_rule` decides how its upstream
/// states aggregate; `skip_if` is then a hard veto applied only once the node
/// would otherwise run (so it can reference upstream results, now available).
fn readiness(flow: &FlowRun, state: &FlowRunState, node: &FlowNode) -> QueueResult<Readiness> {
    let decision = trigger_decision(flow, state, node)?;
    if matches!(decision, Readiness::Ready) {
        if let Some(cond) = &node.skip_if {
            if truthy(&eval_expr(cond, flow, &flow.input, state)?) {
                return Ok(Readiness::Skip);
            }
        }
    }
    Ok(decision)
}

/// Apply the node's `trigger_rule` over the effective state of each incoming
/// edge. An edge is `success` when its source succeeded and the edge is active
/// (Branch case matches / `when` truthy), `failed` when the source failed,
/// `inactive` when the source was skipped or the edge is inactive, or `pending`
/// when the source is not terminal yet.
fn trigger_decision(
    flow: &FlowRun,
    state: &FlowRunState,
    node: &FlowNode,
) -> QueueResult<Readiness> {
    let incoming = flow.spec.incoming(&node.id);
    if incoming.is_empty() {
        // A root has no upstreams; `all_failed` can never be satisfied there.
        return Ok(match node.trigger_rule {
            TriggerRule::AllFailed => Readiness::Skip,
            _ => Readiness::Ready,
        });
    }

    let (mut success, mut failed, mut inactive, mut pending) = (0usize, 0usize, 0usize, 0usize);
    for edge in &incoming {
        match state.get(&edge.from) {
            Some(NodeState::Succeeded { result, .. }) => {
                let active = if is_branch_node(flow, &edge.from) {
                    branch_edge_active(&flow.spec, &edge.from, result, edge)
                } else {
                    match &edge.when {
                        None => true,
                        Some(cond) => truthy(&eval_edge(
                            cond,
                            flow,
                            &flow.input,
                            state,
                            &edge.from,
                            result,
                        )?),
                    }
                };
                if active {
                    success += 1;
                } else {
                    inactive += 1;
                }
            }
            Some(NodeState::Failed { .. }) => failed += 1,
            Some(NodeState::Skipped { .. }) => inactive += 1,
            _ => pending += 1,
        }
    }
    let total = incoming.len();

    Ok(match node.trigger_rule {
        // Every upstream must have succeeded down an active edge.
        TriggerRule::AllSuccess => {
            if failed > 0 || inactive > 0 {
                Readiness::Skip
            } else if pending > 0 {
                Readiness::Wait
            } else {
                Readiness::Ready
            }
        }
        // No failures, at least one success; skipped upstreams tolerated. The
        // rule for joining back together after a Branch.
        TriggerRule::NoneFailedMinOneSuccess => {
            if failed > 0 {
                Readiness::Skip
            } else if pending > 0 {
                Readiness::Wait
            } else if success >= 1 {
                Readiness::Ready
            } else {
                Readiness::Skip
            }
        }
        // Run once every upstream is terminal, regardless of outcome.
        TriggerRule::AllDone => {
            if pending > 0 {
                Readiness::Wait
            } else {
                Readiness::Ready
            }
        }
        // Run only when every upstream failed (a targeted error handler).
        TriggerRule::AllFailed => {
            if success > 0 {
                Readiness::Skip
            } else if pending > 0 {
                Readiness::Wait
            } else if failed == total {
                Readiness::Ready
            } else {
                Readiness::Skip
            }
        }
    })
}

/// Start a ready node (set Running, push child run(s)).
async fn dispatch(
    tx: &mut Transaction<'_, Postgres>,
    flow: &FlowRun,
    state: &mut FlowRunState,
    node: &FlowNode,
) -> QueueResult<()> {
    match &node.body {
        // Branch runs its task once exactly like a leaf; routing happens later
        // (in `readiness`) by matching the task's result against edge cases.
        NodeBody::Script { .. } | NodeBody::Branch { .. } => {
            let child = push_leaf(
                tx,
                flow,
                state,
                leaf_body(node),
                &node.id,
                &flow.input,
                None,
            )
            .await?;
            state.set(NodeState::Running {
                id: node.id.clone(),
                run_id: Some(child),
            });
        }
    }
    Ok(())
}

/// Re-check a Running node against its child run(s).
async fn reconcile_running(
    tx: &mut Transaction<'_, Postgres>,
    flow: &FlowRun,
    state: &mut FlowRunState,
    id: &NodeId,
) -> QueueResult<()> {
    let node = flow
        .spec
        .nodes
        .iter()
        .find(|n| &n.id == id)
        .ok_or_else(|| QueueError::Other(format!("running node '{id}' not in spec")))?;

    let run_id = match state.get(id) {
        Some(NodeState::Running { run_id, .. }) => *run_id,
        _ => return Ok(()),
    };

    // Leaf (Script) or Branch (its task runs as a single leaf).
    let Some(child) = run_id else {
        return Ok(());
    };
    let Some((ok, result)) = child_result(tx, child).await? else {
        return Ok(()); // still running
    };
    if !ok {
        retry_or_fail(tx, flow, state, node, result).await?;
    } else if matches!(&node.body, NodeBody::Branch { .. }) && !is_valid_branch_key(&result) {
        // The branch operator returned a value we can't route on.
        state.set(NodeState::Failed {
            id: id.clone(),
            run_id: Some(child),
            error: branch_result_error(&result),
        });
    } else {
        state.set(NodeState::Succeeded {
            id: id.clone(),
            run_id: Some(child),
            result,
        });
    }
    Ok(())
}

/// Retry a failed leaf node or mark it Failed.
async fn retry_or_fail(
    tx: &mut Transaction<'_, Postgres>,
    flow: &FlowRun,
    state: &mut FlowRunState,
    node: &FlowNode,
    error: Value,
) -> QueueResult<()> {
    // A node uses its own retry policy if set, else the flow-level default
    // (`spec.retry`). To opt out of the flow default, a node sets max_attempts: 0.
    if let Some(policy) = node.retry.as_ref().or(flow.spec.retry.as_ref()) {
        let attempts = state.retries.get(&node.id.0).copied().unwrap_or(0);
        if attempts < policy.max_attempts {
            state.retries.insert(node.id.0.clone(), attempts + 1);
            let delay = backoff_ms(&policy.backoff, attempts + 1);
            let when = if delay == 0 {
                None
            } else {
                Some(chrono::Utc::now() + chrono::Duration::milliseconds(delay as i64))
            };
            let child = push_leaf(
                tx,
                flow,
                state,
                leaf_body(node),
                &node.id,
                &flow.input,
                when,
            )
            .await?;
            state.set(NodeState::Running {
                id: node.id.clone(),
                run_id: Some(child),
            });
            return Ok(());
        }
    }
    state.set(NodeState::Failed {
        id: node.id.clone(),
        run_id: state.get(&node.id).and_then(|s| s.run_id()),
        error,
    });
    Ok(())
}

/// On terminal failure, mark not-yet-started Pending nodes as Skipped so a failed
/// flow's persisted state doesn't leave Pending nodes alongside Skipped ones.
/// Running (in-flight) nodes are left untouched.
fn skip_residual_pending(state: &mut FlowRunState) {
    let ids: Vec<NodeId> = state
        .nodes
        .iter()
        .filter(|s| matches!(s, NodeState::Pending { .. }))
        .map(|s| s.id().clone())
        .collect();
    for id in ids {
        state.set(NodeState::Skipped { id });
    }
}

/// Handle a terminal node failure: run on_error once, else fail the flow.
async fn fail_flow(
    tx: &mut Transaction<'_, Postgres>,
    flow: &FlowRun,
    state: &mut FlowRunState,
) -> QueueResult<FlowProgress> {
    let error = first_failure(state);
    let Some(handler) = &flow.spec.on_error else {
        skip_residual_pending(state);
        return Ok(FlowProgress::Failed { error });
    };
    // Handler already dispatched on an earlier pass → just wait for it.
    if state.on_error.is_some() {
        return Ok(FlowProgress::Suspended);
    }
    // Context handed to the handler as its `flow.input`: which flow run failed and
    // which node ids failed. No error payloads — those are viewed on the run page.
    let failed: Vec<String> = state
        .nodes
        .iter()
        .filter_map(|s| match s {
            NodeState::Failed { id, .. } => Some(id.0.clone()),
            _ => None,
        })
        .collect();
    let context = serde_json::json!({ "flow_run_id": flow.id, "failed": failed });
    match push_handler(tx, flow, state, handler, &context).await {
        Ok(child) => {
            state.on_error = Some(NodeState::Running {
                id: handler.id.clone(),
                run_id: Some(child),
            });
            Ok(FlowProgress::Suspended)
        }
        // Infra error → abort (rolls back, retried). A node-level error means the
        // handler itself can't be dispatched (e.g. its script was deleted); record
        // it Failed and fail the flow rather than rolling back the pass.
        Err(QueueError::Db(e)) => Err(QueueError::Db(e)),
        Err(e) => {
            state.on_error = Some(NodeState::Failed {
                id: handler.id.clone(),
                run_id: None,
                error: node_error_value(&e),
            });
            skip_residual_pending(state);
            Ok(FlowProgress::Failed { error })
        }
    }
}

/// Returns true once the on_error handler reaches a terminal state.
async fn reconcile_handler(
    tx: &mut Transaction<'_, Postgres>,
    state: &mut FlowRunState,
    handler: &FlowNode,
) -> QueueResult<bool> {
    let Some(NodeState::Running {
        run_id: Some(child),
        ..
    }) = &state.on_error
    else {
        return Ok(true);
    };
    let child = *child;
    match child_result(tx, child).await? {
        None => Ok(false),
        Some((ok, res)) => {
            state.on_error = Some(if ok {
                NodeState::Succeeded {
                    id: handler.id.clone(),
                    run_id: Some(child),
                    result: res,
                }
            } else {
                NodeState::Failed {
                    id: handler.id.clone(),
                    run_id: Some(child),
                    error: res,
                }
            });
            Ok(true)
        }
    }
}

/// Wake the parent flow of a just-finished child run. Serialized with
/// `advance_flow` via `run_flow_status FOR UPDATE` so the wake is never lost.
#[tracing::instrument(name = "queue::on_child_complete", skip(db), fields(%child_run_id))]
pub async fn on_child_complete(db: &PgPool, child_run_id: Uuid) -> QueueResult<()> {
    let parent = sqlx::query_scalar!("SELECT parent_run FROM run WHERE id = $1", child_run_id)
        .fetch_optional(db)
        .await?
        .flatten();
    let Some(parent_id) = parent else {
        return Ok(());
    };

    let mut tx = db.begin().await?;
    let _lock = sqlx::query_scalar!(
        r#"SELECT 1 AS "one!" FROM run_flow_status WHERE run_id = $1 FOR UPDATE"#,
        parent_id
    )
    .fetch_optional(&mut *tx)
    .await?;
    let woke = sqlx::query!(
        "UPDATE run_queue SET scheduled_for = now()
         WHERE id = $1
           AND EXISTS (SELECT 1 FROM run r WHERE r.id = $1 AND r.kind IN ('flow', 'flow_preview'))",
        parent_id
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    if woke.rows_affected() > 0 {
        tracing::debug!(parent = %parent_id, "woke parent flow");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn first_failure(state: &FlowRunState) -> Value {
    for s in &state.nodes {
        if let NodeState::Failed { id, error, .. } = s {
            return serde_json::json!({ "failed_node": id.0, "error": error });
        }
    }
    Value::Null
}

/// Flow result: the sink nodes' results (no outgoing edges). One sink → its
/// result directly; several → an object keyed by node id.
fn flow_result(flow: &FlowRun, state: &FlowRunState) -> Value {
    let has_out: std::collections::HashSet<&str> =
        flow.spec.edges.iter().map(|e| e.from.0.as_str()).collect();
    let sinks: Vec<&FlowNode> = flow
        .spec
        .nodes
        .iter()
        .filter(|n| !has_out.contains(n.id.0.as_str()))
        .collect();
    if sinks.len() == 1 {
        return node_result(state, &sinks[0].id);
    }
    let mut m = Map::new();
    for n in sinks {
        m.insert(n.id.0.clone(), node_result(state, &n.id));
    }
    Value::Object(m)
}

fn node_result(state: &FlowRunState, id: &NodeId) -> Value {
    match state.get(id) {
        Some(NodeState::Succeeded { result, .. }) => result.clone(),
        _ => Value::Null,
    }
}

async fn load_or_init_state(
    tx: &mut Transaction<'_, Postgres>,
    flow_run_id: Uuid,
    spec: &FlowSpec,
) -> QueueResult<FlowRunState> {
    let row = sqlx::query_scalar!(
        "SELECT flow_status FROM run_flow_status WHERE run_id = $1 FOR UPDATE",
        flow_run_id
    )
    .fetch_optional(&mut **tx)
    .await?;
    match row {
        Some(v) => serde_json::from_value(v)
            .map_err(|e| QueueError::Other(format!("invalid flow_status: {e}"))),
        None => Ok(FlowRunState::init(spec.nodes.iter().map(|n| n.id.clone()))),
    }
}

async fn save_state(
    tx: &mut Transaction<'_, Postgres>,
    flow_run_id: Uuid,
    state: &FlowRunState,
) -> QueueResult<()> {
    let json = serde_json::to_value(state)
        .map_err(|e| QueueError::Other(format!("serialize flow_status: {e}")))?;
    sqlx::query!(
        "INSERT INTO run_flow_status (run_id, flow_status) VALUES ($1, $2)
         ON CONFLICT (run_id) DO UPDATE SET flow_status = $2",
        flow_run_id,
        json
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn child_result(
    tx: &mut Transaction<'_, Postgres>,
    run_id: Uuid,
) -> QueueResult<Option<(bool, Value)>> {
    let row = sqlx::query!(
        "SELECT success, result FROM run_completed WHERE id = $1",
        run_id
    )
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.map(|r| (r.success, r.result.unwrap_or(Value::Null))))
}

fn backoff_ms(backoff: &Backoff, attempt: u32) -> u64 {
    match backoff {
        Backoff::Fixed { delay_ms } => *delay_ms,
        Backoff::Exponential {
            base_ms, factor, ..
        } => base_ms.saturating_mul((*factor as u64).saturating_pow(attempt.saturating_sub(1))),
    }
}

fn eval_binding(
    b: &InputBinding,
    flow: &FlowRun,
    input: &Value,
    state: &FlowRunState,
) -> QueueResult<Value> {
    match b {
        InputBinding::Static { value } => Ok(value.clone()),
        InputBinding::Expr { expr } => eval_expr(expr, flow, input, state),
    }
}

fn eval_expr(
    expr: &Expr,
    flow: &FlowRun,
    input: &Value,
    state: &FlowRunState,
) -> QueueResult<Value> {
    let ctx = expr_context(flow, input, state);
    coveflow_flow_expr::eval_str(&expr.0, &ctx)
        .map_err(|e| QueueError::Other(format!("expression '{}': {e}", expr.0)))
}

/// Evaluate an edge condition; the source node's result is available as
/// `steps.<source>.result` like any other node.
fn eval_edge(
    expr: &Expr,
    flow: &FlowRun,
    input: &Value,
    state: &FlowRunState,
    _source: &NodeId,
    _source_result: &Value,
) -> QueueResult<Value> {
    eval_expr(expr, flow, input, state)
}

/// Build the JSON context the expression language navigates: `flow.input`, every
/// succeeded node's result under `steps.<id>.result`, and the Airflow-style
/// `run.*` execution context (logical date, interval, schedule meta). `input` is
/// usually the flow's input, but the on_error handler passes the failure context.
fn expr_context(flow: &FlowRun, input: &Value, state: &FlowRunState) -> Value {
    serde_json::json!({
        "flow": { "input": input },
        "steps": Value::Object(state.succeeded_steps()),
        "run": flow.run_context_value(state),
    })
}

fn transform_inputs(
    inputs: &HashMap<String, InputBinding>,
    flow: &FlowRun,
    input: &Value,
    state: &FlowRunState,
) -> QueueResult<Value> {
    let mut m = Map::new();
    for (k, b) in inputs {
        m.insert(k.clone(), eval_binding(b, flow, input, state)?);
    }
    Ok(Value::Object(m))
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(false),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

// ---------------------------------------------------------------------------
// Branch routing helpers
// ---------------------------------------------------------------------------

/// The body actually dispatched for a node: a Branch runs its inner `task`
/// (a Script), every other body runs itself.
fn leaf_body(node: &FlowNode) -> &NodeBody {
    match &node.body {
        NodeBody::Branch { task } => task.as_ref(),
        other => other,
    }
}

fn is_branch_node(flow: &FlowRun, id: &NodeId) -> bool {
    flow.spec
        .nodes
        .iter()
        .any(|n| &n.id == id && matches!(n.body, NodeBody::Branch { .. }))
}

/// A branch operator's result must be a scalar (string/number/bool) or an array
/// of scalars; anything else (object, null, nested) cannot be routed on.
fn is_valid_branch_key(v: &Value) -> bool {
    fn is_scalar(v: &Value) -> bool {
        matches!(v, Value::String(_) | Value::Number(_) | Value::Bool(_))
    }
    match v {
        Value::Array(a) => a.iter().all(is_scalar),
        other => is_scalar(other),
    }
}

fn branch_result_error(v: &Value) -> Value {
    serde_json::json!({
        "error": "branch result must be a scalar or array of scalars",
        "result": v,
    })
}

/// The set of routing keys a branch result yields: a scalar is a one-element
/// set, an array is its elements.
fn branch_result_keys(result: &Value) -> Vec<&Value> {
    match result {
        Value::Array(a) => a.iter().collect(),
        other => vec![other],
    }
}

/// Equality used for branch routing. Numbers compare by value so an operator
/// returning `1.0` still matches a case authored as `1` (serde_json's `Number`
/// PartialEq otherwise distinguishes int vs float representations). Other types
/// use plain JSON equality (a string never matches a number).
fn branch_key_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => match (x.as_f64(), y.as_f64()) {
            (Some(xf), Some(yf)) => xf == yf,
            _ => x == y,
        },
        _ => a == b,
    }
}

/// Is `edge` (out of branch `from`, which returned `result`) active? A `Match`
/// edge is active when its value is among the result keys; the `Default` edge is
/// active only when no sibling `Match` matched.
fn branch_edge_active(spec: &FlowSpec, from: &NodeId, result: &Value, edge: &FlowEdge) -> bool {
    let keys = branch_result_keys(result);
    match &edge.case {
        Some(BranchCase::Match { value }) => keys.iter().any(|k| branch_key_eq(k, value)),
        Some(BranchCase::Default) => !spec.edges.iter().any(|e| {
            &e.from == from
                && matches!(&e.case, Some(BranchCase::Match { value }) if keys.iter().any(|k| branch_key_eq(k, value)))
        }),
        None => false, // validation forbids; treat as inactive defensively
    }
}

/// Resolve a Script node's stable id to its current `(path, hash)`. A node may
/// pin an explicit hash; otherwise the latest version is used. The path is the
/// script's current location (a denormalized snapshot stored on the child run for
/// display/history). The worker fetches content by hash.
async fn resolve_script(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: &str,
    script_id: Uuid,
    explicit: Option<&str>,
) -> QueueResult<(String, String)> {
    let row = sqlx::query!(
        "SELECT path, hash FROM script WHERE workspace_id = $1 AND script_id = $2
         ORDER BY created_at DESC LIMIT 1",
        workspace_id,
        script_id,
    )
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| {
        QueueError::Other(format!(
            "referenced script {script_id} not found — it may have been deleted; \
             reassign or remove this node in the flow editor"
        ))
    })?;
    let hash = explicit.map(str::to_string).unwrap_or(row.hash);
    Ok((row.path, hash))
}

/// Insert a child run (run + run_queue) within the flow's transaction.
#[allow(clippy::too_many_arguments)]
async fn insert_child(
    tx: &mut Transaction<'_, Postgres>,
    flow: &FlowRun,
    kind: &str,
    script_path: Option<&str>,
    script_hash: Option<&str>,
    raw_code: Option<&str>,
    language: Option<&str>,
    args: Option<Value>,
    flow_value: Option<Value>,
    queue: Option<&str>,
    node_id: &NodeId,
    scheduled_for: Option<chrono::DateTime<chrono::Utc>>,
) -> QueueResult<Uuid> {
    let id = Uuid::new_v4();
    let tag = queue.unwrap_or(&flow.tag);
    sqlx::query!(
        "INSERT INTO run (id, workspace_id, kind, script_path, script_hash,
            raw_code, language, args, flow_value, tag, parent_run, root_run,
            flow_step_id, created_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)",
        id,
        flow.workspace_id,
        kind,
        script_path,
        script_hash,
        raw_code,
        language,
        args as Option<Value>,
        flow_value as Option<Value>,
        tag,
        flow.id,
        flow.root,
        node_id.0,
        flow.created_by,
    )
    .execute(&mut **tx)
    .await?;
    sqlx::query!(
        "INSERT INTO run_queue (id, scheduled_for, tag) VALUES ($1, COALESCE($2, now()), $3)",
        id,
        scheduled_for,
        tag
    )
    .execute(&mut **tx)
    .await?;
    Ok(id)
}

/// Push a leaf (Script) child run for a node, evaluating its inputs.
async fn push_leaf(
    tx: &mut Transaction<'_, Postgres>,
    flow: &FlowRun,
    state: &FlowRunState,
    body: &NodeBody,
    node_id: &NodeId,
    flow_input: &Value,
    scheduled_for: Option<chrono::DateTime<chrono::Utc>>,
) -> QueueResult<Uuid> {
    match body {
        NodeBody::Script {
            script_id,
            hash,
            inputs,
            queue,
        } => {
            let args = transform_inputs(inputs, flow, flow_input, state)?;
            let (path, resolved) =
                resolve_script(tx, &flow.workspace_id, *script_id, hash.as_deref()).await?;
            insert_child(
                tx,
                flow,
                "script",
                Some(&path),
                Some(&resolved),
                None,
                None,
                Some(args),
                None,
                queue.as_deref(),
                node_id,
                scheduled_for,
            )
            .await
        }
        _ => Err(QueueError::Other("push_leaf expects a Script body".into())),
    }
}

/// Dispatch the on_error handler with the failure as its input.
async fn push_handler(
    tx: &mut Transaction<'_, Postgres>,
    flow: &FlowRun,
    state: &FlowRunState,
    handler: &FlowNode,
    context: &Value,
) -> QueueResult<Uuid> {
    // The handler runs with `context` as its `flow.input` and the live `state`,
    // so its input bindings can pull the failure context (`flow.input.failed`,
    // `flow.input.flow_run_id`) and any succeeded `steps.<id>.result`.
    match &handler.body {
        NodeBody::Script { .. } => {
            push_leaf(tx, flow, state, &handler.body, &handler.id, context, None).await
        }
        _ => Err(QueueError::Other(
            "on_error handler must be a Script node".into(),
        )),
    }
}
