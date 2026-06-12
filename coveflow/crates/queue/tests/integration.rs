use coveflow_queue::{
    CancelOutcome, NewRun, QueueError, RetentionConfig, RunLogChunk, ServiceLogChunk,
    append_run_log_chunks, append_service_log_chunks, cancel_run, cancel_run_tree, check_cancel,
    claim_run, execute_retention, finish_run, get_run_log_chunks, get_service_log_chunks,
    mark_fail, mark_success, reap_lost_workers, rerun, submit_run,
};
use coveflow_types::{RunKind, ScriptLang};
use sqlx::PgPool;

/// Helper: insert a workspace so FK constraints pass.
async fn setup_workspace(pool: &PgPool, workspace_id: &str) {
    sqlx::query!(
        "INSERT INTO workspace (id, name, owner) VALUES ($1, $2, $3)
         ON CONFLICT DO NOTHING",
        workspace_id,
        "Test Workspace",
        "test@example.com"
    )
    .execute(pool)
    .await
    .expect("failed to create workspace");
}

/// Helper: insert a team + team_quota for quota tests.
async fn setup_team_quota(
    pool: &PgPool,
    workspace_id: &str,
    team: &str,
    max_concurrent: Option<i32>,
    max_cpus: Option<f32>,
    max_memory_mb: Option<i64>,
    max_daily: Option<i32>,
) {
    sqlx::query!(
        "INSERT INTO team (workspace_id, name) VALUES ($1, $2)
         ON CONFLICT DO NOTHING",
        workspace_id,
        team
    )
    .execute(pool)
    .await
    .expect("failed to create team");

    sqlx::query!(
        "INSERT INTO team_quota (workspace_id, team_name, max_concurrent_runs, max_cpus, max_memory_mb, max_daily_runs)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (workspace_id, team_name) DO UPDATE
         SET max_concurrent_runs = $3, max_cpus = $4, max_memory_mb = $5, max_daily_runs = $6",
        workspace_id,
        team,
        max_concurrent,
        max_cpus,
        max_memory_mb,
        max_daily,
    )
    .execute(pool)
    .await
    .expect("failed to create team_quota");
}

async fn set_workspace_log_retention(pool: &PgPool, workspace_id: &str, days: i32) {
    sqlx::query!(
        "INSERT INTO workspace_settings (workspace_id, log_retention_days)
         VALUES ($1, $2)
         ON CONFLICT (workspace_id) DO UPDATE
         SET log_retention_days = EXCLUDED.log_retention_days",
        workspace_id,
        days,
    )
    .execute(pool)
    .await
    .expect("failed to set workspace log retention");
}

async fn set_team_log_retention(pool: &PgPool, workspace_id: &str, team: &str, days: i32) {
    sqlx::query!(
        "UPDATE team_quota
         SET log_retention_days = $3
         WHERE workspace_id = $1 AND team_name = $2",
        workspace_id,
        team,
        days,
    )
    .execute(pool)
    .await
    .expect("failed to set team log retention");
}

async fn append_run_log_at(
    pool: &PgPool,
    run_id: uuid::Uuid,
    seq: i32,
    created_at: chrono::DateTime<chrono::Utc>,
) {
    append_run_log_chunks(
        pool,
        &[RunLogChunk {
            run_id,
            seq,
            min_level: 3,
            max_level: 3,
            line_count: 1,
            entries: serde_json::json!([{"msg": format!("run log {seq}")}]),
        }],
    )
    .await
    .unwrap();

    sqlx::query!(
        "UPDATE run_log SET created_at = $1 WHERE run_id = $2 AND seq = $3",
        created_at,
        run_id,
        seq,
    )
    .execute(pool)
    .await
    .unwrap();
}

async fn run_log_count(pool: &PgPool, run_id: uuid::Uuid) -> i64 {
    sqlx::query_scalar!(
        r#"SELECT COUNT(*) as "count!" FROM run_log WHERE run_id = $1"#,
        run_id
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

fn retention_config_without_sleep() -> RetentionConfig {
    RetentionConfig {
        batch_sleep: std::time::Duration::from_millis(0),
        ..RetentionConfig::default()
    }
}

fn default_run(workspace_id: &str) -> NewRun<'_> {
    NewRun {
        workspace_id,
        kind: RunKind::Script,
        script_hash: None,
        script_path: Some("users/test/hello"),
        raw_code: None,
        language: Some(ScriptLang::Python3),
        args: Some(serde_json::json!({"x": 1})),
        flow_value: None,
        tag: "default",
        parent_run: None,
        root_run: None,
        flow_step_id: None,
        team_owner: None,
        created_by: "test@example.com",
        trace_id: None,
        span_id: None,
        scheduled_for: None,
        priority: None,
        cpus: None,
        memory_mb: None,
        disk_mb: None,
        requirements: vec![],
        timeout: None,
        custom_image: None,
        schedule_id: None,
        scheduled_time: None,
        data_interval_end: None,
        trigger_id: None,
        trigger_context: None,
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn test_submit_run(pool: PgPool) {
    setup_workspace(&pool, "ws-1").await;

    let run_id = submit_run(&pool, default_run("ws-1")).await.unwrap();

    // Verify run exists in DB
    let row = sqlx::query!("SELECT id, kind, tag FROM run WHERE id = $1", run_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.kind, "script");
    assert_eq!(row.tag, "default");

    // Verify run_queue entry exists
    let queue_row = sqlx::query!("SELECT running FROM run_queue WHERE id = $1", run_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(!queue_row.running);
}

#[sqlx::test(migrations = "../../migrations")]
async fn test_claim_run(pool: PgPool) {
    setup_workspace(&pool, "ws-1").await;

    let run_id = submit_run(&pool, default_run("ws-1")).await.unwrap();

    // Register worker
    sqlx::query!(
        "INSERT INTO worker_ping (worker, tags) VALUES ($1, $2)",
        "worker-1",
        &["default".to_string()] as &[String]
    )
    .execute(&pool)
    .await
    .unwrap();

    // Claim
    let active = claim_run(&pool, "worker-1", &["default".into()], 4.0, 4096, 10240)
        .await
        .unwrap();

    let active = active.expect("should have claimed a run");
    assert_eq!(active.run.id, run_id);
    assert_eq!(active.tag, "default");
    assert_eq!(active.cpus, 1.0);
    assert_eq!(active.memory_mb, 512);
    assert_eq!(active.disk_mb, 1024);

    // Verify run_queue updated
    let queue_row = sqlx::query!(
        "SELECT running, worker FROM run_queue WHERE id = $1",
        run_id
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(queue_row.running);
    assert_eq!(queue_row.worker.as_deref(), Some("worker-1"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn test_claim_run_none_available(pool: PgPool) {
    setup_workspace(&pool, "ws-1").await;

    // No runs submitted, nothing to claim
    let active = claim_run(&pool, "worker-1", &["default".into()], 4.0, 4096, 10240)
        .await
        .unwrap();
    assert!(active.is_none());
}

#[sqlx::test(migrations = "../../migrations")]
async fn test_claim_run_respects_tag(pool: PgPool) {
    setup_workspace(&pool, "ws-1").await;

    // Submit run with tag "gpu"
    let mut run = default_run("ws-1");
    run.tag = "gpu";
    submit_run(&pool, run).await.unwrap();

    // Try to claim with tag "default" — should get nothing
    let active = claim_run(&pool, "worker-1", &["default".into()], 4.0, 4096, 10240)
        .await
        .unwrap();
    assert!(active.is_none());

    // Claim with tag "gpu" — should succeed
    let active = claim_run(&pool, "worker-1", &["gpu".into()], 4.0, 4096, 10240)
        .await
        .unwrap();
    assert!(active.is_some());
}

#[sqlx::test(migrations = "../../migrations")]
async fn test_claim_run_respects_resource_limits(pool: PgPool) {
    setup_workspace(&pool, "ws-1").await;

    // Submit run needing 2 cpus, 1024 MB
    let mut run = default_run("ws-1");
    run.cpus = Some(2.0);
    run.memory_mb = Some(1024);
    submit_run(&pool, run).await.unwrap();

    // Worker with only 1 cpu — can't claim
    let active = claim_run(&pool, "worker-1", &["default".into()], 1.0, 4096, 10240)
        .await
        .unwrap();
    assert!(active.is_none());

    // Worker with only 512 MB — can't claim
    let active = claim_run(&pool, "worker-1", &["default".into()], 4.0, 512, 10240)
        .await
        .unwrap();
    assert!(active.is_none());

    // Worker with enough resources — can claim
    let active = claim_run(&pool, "worker-1", &["default".into()], 4.0, 2048, 10240)
        .await
        .unwrap();
    assert!(active.is_some());
}

#[sqlx::test(migrations = "../../migrations")]
async fn test_finish_run(pool: PgPool) {
    setup_workspace(&pool, "ws-1").await;

    let run_id = submit_run(&pool, default_run("ws-1")).await.unwrap();

    // Register worker and claim
    sqlx::query!(
        "INSERT INTO worker_ping (worker, tags) VALUES ($1, $2)",
        "worker-1",
        &["default".to_string()] as &[String]
    )
    .execute(&pool)
    .await
    .unwrap();
    claim_run(&pool, "worker-1", &["default".into()], 4.0, 4096, 10240)
        .await
        .unwrap();

    // Finish
    let result = serde_json::json!({"output": "hello"});
    finish_run(&pool, run_id, true, result.clone(), 150, 1024 * 1024, None)
        .await
        .unwrap();

    // Verify run_completed exists
    let completed = sqlx::query!(
        "SELECT success, duration_ms, result FROM run_completed WHERE id = $1",
        run_id
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(completed.success);
    assert_eq!(completed.duration_ms, 150);
    assert_eq!(completed.result, Some(result));

    // Verify removed from run_queue
    let queue = sqlx::query!("SELECT id FROM run_queue WHERE id = $1", run_id)
        .fetch_optional(&pool)
        .await
        .unwrap();
    assert!(queue.is_none());

    // Verify worker_ping updated
    let worker = sqlx::query!(
        "SELECT current_run_id, runs_completed FROM worker_ping WHERE worker = $1",
        "worker-1"
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(worker.current_run_id.is_none());
    assert_eq!(worker.runs_completed, Some(1));
}

#[sqlx::test(migrations = "../../migrations")]
async fn test_submit_run_quota_concurrent_limit(pool: PgPool) {
    setup_workspace(&pool, "ws-1").await;
    setup_team_quota(&pool, "ws-1", "ml-team", Some(1), None, None, None).await;

    // Submit first run (team_owner = ml-team)
    let mut run1 = default_run("ws-1");
    run1.team_owner = Some("ml-team");
    let id1 = submit_run(&pool, run1).await.unwrap();

    // Mark it as running
    sqlx::query!("UPDATE run_queue SET running = TRUE WHERE id = $1", id1)
        .execute(&pool)
        .await
        .unwrap();

    // Submit second run — should be rejected (max_concurrent_runs = 1)
    let mut run2 = default_run("ws-1");
    run2.team_owner = Some("ml-team");
    let err = submit_run(&pool, run2).await.unwrap_err();

    match err {
        QueueError::QuotaExceeded(msg) => {
            assert!(msg.contains("concurrent run limit"));
        }
        other => panic!("expected QuotaExceeded, got: {other}"),
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn test_submit_run_quota_cpu_limit(pool: PgPool) {
    setup_workspace(&pool, "ws-1").await;
    setup_team_quota(&pool, "ws-1", "ml-team", None, Some(4.0), None, None).await;

    // Submit run using 3 cpus
    let mut run1 = default_run("ws-1");
    run1.team_owner = Some("ml-team");
    run1.cpus = Some(3.0);
    let id1 = submit_run(&pool, run1).await.unwrap();

    // Mark running
    sqlx::query!("UPDATE run_queue SET running = TRUE WHERE id = $1", id1)
        .execute(&pool)
        .await
        .unwrap();

    // Submit run needing 2 cpus — should be rejected (3 + 2 > 4)
    let mut run2 = default_run("ws-1");
    run2.team_owner = Some("ml-team");
    run2.cpus = Some(2.0);
    let err = submit_run(&pool, run2).await.unwrap_err();

    match err {
        QueueError::QuotaExceeded(msg) => {
            assert!(msg.contains("CPU quota exceeded"));
        }
        other => panic!("expected QuotaExceeded, got: {other}"),
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn test_priority_ordering(pool: PgPool) {
    setup_workspace(&pool, "ws-1").await;

    // Submit low priority first
    let mut low = default_run("ws-1");
    low.priority = Some(0);
    let low_id = submit_run(&pool, low).await.unwrap();

    // Submit high priority second
    let mut high = default_run("ws-1");
    high.priority = Some(10);
    let _high_id = submit_run(&pool, high).await.unwrap();

    // Claim — should get high priority first
    let active = claim_run(&pool, "worker-1", &["default".into()], 4.0, 4096, 10240)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(
        active.run.id, low_id,
        "should claim high-priority run first"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn test_cancel_queued_run(pool: PgPool) {
    setup_workspace(&pool, "ws-1").await;

    let run_id = submit_run(&pool, default_run("ws-1")).await.unwrap();

    // Cancel a queued (not running) run — should complete immediately
    let outcome = cancel_run(
        &pool,
        run_id,
        "admin@test.com",
        Some("no longer needed"),
        false,
    )
    .await
    .unwrap();
    assert_eq!(outcome, CancelOutcome::CompletedImmediately);

    // Verify removed from run_queue
    let queue = sqlx::query!("SELECT id FROM run_queue WHERE id = $1", run_id)
        .fetch_optional(&pool)
        .await
        .unwrap();
    assert!(queue.is_none());

    // Verify run_completed created with canceled_by
    let completed = sqlx::query!(
        "SELECT success, canceled_by, canceled_reason FROM run_completed WHERE id = $1",
        run_id
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!completed.success);
    assert_eq!(completed.canceled_by.as_deref(), Some("admin@test.com"));
    assert_eq!(
        completed.canceled_reason.as_deref(),
        Some("no longer needed")
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn test_cancel_running_run(pool: PgPool) {
    setup_workspace(&pool, "ws-1").await;

    let run_id = submit_run(&pool, default_run("ws-1")).await.unwrap();

    // Mark as running
    sqlx::query!(
        "UPDATE run_queue SET running = TRUE, worker = 'worker-1' WHERE id = $1",
        run_id
    )
    .execute(&pool)
    .await
    .unwrap();

    // Cancel a running run — should set flag only
    let outcome = cancel_run(&pool, run_id, "admin@test.com", None, false)
        .await
        .unwrap();
    assert_eq!(outcome, CancelOutcome::FlagSet);

    // Verify still in run_queue with cancel flag set
    let row = sqlx::query!(
        "SELECT canceled_by, cancel_requested_at FROM run_queue WHERE id = $1",
        run_id
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.canceled_by.as_deref(), Some("admin@test.com"));
    assert!(row.cancel_requested_at.is_some());
}

#[sqlx::test(migrations = "../../migrations")]
async fn test_cancel_already_completed(pool: PgPool) {
    setup_workspace(&pool, "ws-1").await;

    let run_id = submit_run(&pool, default_run("ws-1")).await.unwrap();

    // Finish the run first
    sqlx::query!(
        "INSERT INTO worker_ping (worker, tags) VALUES ($1, $2)",
        "worker-1",
        &["default".to_string()] as &[String]
    )
    .execute(&pool)
    .await
    .unwrap();
    claim_run(&pool, "worker-1", &["default".into()], 4.0, 4096, 10240)
        .await
        .unwrap();
    finish_run(
        &pool,
        run_id,
        true,
        serde_json::json!({"ok": true}),
        100,
        0,
        None,
    )
    .await
    .unwrap();

    // Try to cancel — should report already completed
    let outcome = cancel_run(&pool, run_id, "admin@test.com", None, false)
        .await
        .unwrap();
    assert_eq!(outcome, CancelOutcome::AlreadyCompleted);
}

#[sqlx::test(migrations = "../../migrations")]
async fn test_cancel_not_found(pool: PgPool) {
    setup_workspace(&pool, "ws-1").await;

    let fake_id = uuid::Uuid::new_v4();
    let outcome = cancel_run(&pool, fake_id, "admin@test.com", None, false)
        .await
        .unwrap();
    assert_eq!(outcome, CancelOutcome::NotFound);
}

#[sqlx::test(migrations = "../../migrations")]
async fn test_cancel_idempotent(pool: PgPool) {
    setup_workspace(&pool, "ws-1").await;

    let run_id = submit_run(&pool, default_run("ws-1")).await.unwrap();

    // Mark as running
    sqlx::query!(
        "UPDATE run_queue SET running = TRUE, worker = 'worker-1' WHERE id = $1",
        run_id
    )
    .execute(&pool)
    .await
    .unwrap();

    // Cancel twice — both should succeed
    let outcome1 = cancel_run(&pool, run_id, "admin@test.com", None, false)
        .await
        .unwrap();
    assert_eq!(outcome1, CancelOutcome::FlagSet);

    let outcome2 = cancel_run(&pool, run_id, "other@test.com", None, false)
        .await
        .unwrap();
    // Second cancel also sets the flag (overwrites), still FlagSet
    assert_eq!(outcome2, CancelOutcome::FlagSet);
}

#[sqlx::test(migrations = "../../migrations")]
async fn test_check_cancel_not_canceled(pool: PgPool) {
    setup_workspace(&pool, "ws-1").await;

    let run_id = submit_run(&pool, default_run("ws-1")).await.unwrap();

    // No cancel requested — should return None
    let result = check_cancel(&pool, run_id).await.unwrap();
    assert!(result.is_none());
}

#[sqlx::test(migrations = "../../migrations")]
async fn test_check_cancel_after_cancel(pool: PgPool) {
    setup_workspace(&pool, "ws-1").await;

    let run_id = submit_run(&pool, default_run("ws-1")).await.unwrap();

    // Mark running and cancel
    sqlx::query!("UPDATE run_queue SET running = TRUE WHERE id = $1", run_id)
        .execute(&pool)
        .await
        .unwrap();

    cancel_run(&pool, run_id, "admin@test.com", Some("testing"), false)
        .await
        .unwrap();

    // check_cancel should return the canceler info
    let result = check_cancel(&pool, run_id).await.unwrap();
    assert!(result.is_some());
    let (by, reason) = result.unwrap();
    assert_eq!(by, "admin@test.com");
    assert_eq!(reason.as_deref(), Some("testing"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn test_force_cancel_running_run(pool: PgPool) {
    setup_workspace(&pool, "ws-1").await;

    let run_id = submit_run(&pool, default_run("ws-1")).await.unwrap();

    // Mark as running
    sqlx::query!(
        "UPDATE run_queue SET running = TRUE, worker = 'worker-1' WHERE id = $1",
        run_id
    )
    .execute(&pool)
    .await
    .unwrap();

    // Force cancel — should complete immediately even though running
    let outcome = cancel_run(&pool, run_id, "admin@test.com", Some("zombie"), true)
        .await
        .unwrap();
    assert_eq!(outcome, CancelOutcome::CompletedImmediately);

    // Verify removed from queue
    let queue = sqlx::query!("SELECT id FROM run_queue WHERE id = $1", run_id)
        .fetch_optional(&pool)
        .await
        .unwrap();
    assert!(queue.is_none());

    // Verify run_completed
    let completed = sqlx::query!(
        "SELECT success, canceled_by FROM run_completed WHERE id = $1",
        run_id
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!completed.success);
    assert_eq!(completed.canceled_by.as_deref(), Some("admin@test.com"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn test_force_cancel_not_found(pool: PgPool) {
    setup_workspace(&pool, "ws-1").await;

    let fake_id = uuid::Uuid::new_v4();
    let outcome = cancel_run(&pool, fake_id, "admin@test.com", None, true)
        .await
        .unwrap();
    assert_eq!(outcome, CancelOutcome::NotFound);
}

#[sqlx::test(migrations = "../../migrations")]
async fn test_cancel_run_tree(pool: PgPool) {
    setup_workspace(&pool, "ws-1").await;

    // Create parent run
    let parent_id = submit_run(&pool, default_run("ws-1")).await.unwrap();

    // Create child runs with parent_run pointing to parent
    let mut child1 = default_run("ws-1");
    child1.parent_run = Some(parent_id);
    let child1_id = submit_run(&pool, child1).await.unwrap();

    let mut child2 = default_run("ws-1");
    child2.parent_run = Some(parent_id);
    let child2_id = submit_run(&pool, child2).await.unwrap();

    // Create grandchild
    let mut grandchild = default_run("ws-1");
    grandchild.parent_run = Some(child1_id);
    let grandchild_id = submit_run(&pool, grandchild).await.unwrap();

    // Cancel the whole tree
    let results = cancel_run_tree(&pool, parent_id, "admin@test.com", Some("cancel tree"))
        .await
        .unwrap();

    // Should have canceled 4 runs (parent + 2 children + 1 grandchild)
    assert_eq!(results.len(), 4);

    // All should be CompletedImmediately (none were running)
    for (_, outcome) in &results {
        assert_eq!(*outcome, CancelOutcome::CompletedImmediately);
    }

    // Verify all removed from queue
    for id in [parent_id, child1_id, child2_id, grandchild_id] {
        let queue = sqlx::query!("SELECT id FROM run_queue WHERE id = $1", id)
            .fetch_optional(&pool)
            .await
            .unwrap();
        assert!(queue.is_none(), "run {id} should be removed from queue");
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn test_claim_skips_canceled_run(pool: PgPool) {
    setup_workspace(&pool, "ws-1").await;

    let run_id = submit_run(&pool, default_run("ws-1")).await.unwrap();

    // Set cancel flag on the queued run (simulate a cancel on a queued run
    // that hasn't been cleaned up yet — edge case)
    sqlx::query!(
        "UPDATE run_queue SET canceled_by = 'admin' WHERE id = $1",
        run_id
    )
    .execute(&pool)
    .await
    .unwrap();

    // Try to claim — should get nothing because it's canceled
    let active = claim_run(&pool, "worker-1", &["default".into()], 4.0, 4096, 10240)
        .await
        .unwrap();
    assert!(active.is_none());
}

#[sqlx::test(migrations = "../../migrations")]
async fn test_mark_success(pool: PgPool) {
    setup_workspace(&pool, "ws-1").await;

    let run_id = submit_run(&pool, default_run("ws-1")).await.unwrap();

    // Mark as success
    let result_val = serde_json::json!({"output": "manually approved"});
    mark_success(
        &pool,
        run_id,
        "admin@test.com",
        Some("manual approval"),
        Some(result_val.clone()),
    )
    .await
    .unwrap();

    // Verify run_completed
    let completed = sqlx::query!(
        "SELECT success, marked_by, mark_reason, result FROM run_completed WHERE id = $1",
        run_id
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(completed.success);
    assert_eq!(completed.marked_by.as_deref(), Some("admin@test.com"));
    assert_eq!(completed.mark_reason.as_deref(), Some("manual approval"));
    assert_eq!(completed.result, Some(result_val));

    // Verify removed from queue
    let queue = sqlx::query!("SELECT id FROM run_queue WHERE id = $1", run_id)
        .fetch_optional(&pool)
        .await
        .unwrap();
    assert!(queue.is_none());
}

#[sqlx::test(migrations = "../../migrations")]
async fn test_mark_fail(pool: PgPool) {
    setup_workspace(&pool, "ws-1").await;

    let run_id = submit_run(&pool, default_run("ws-1")).await.unwrap();

    // Mark as fail
    mark_fail(
        &pool,
        run_id,
        "admin@test.com",
        Some("known bad output"),
        None,
    )
    .await
    .unwrap();

    // Verify run_completed
    let completed = sqlx::query!(
        "SELECT success, marked_by, mark_reason FROM run_completed WHERE id = $1",
        run_id
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!completed.success);
    assert_eq!(completed.marked_by.as_deref(), Some("admin@test.com"));
    assert_eq!(completed.mark_reason.as_deref(), Some("known bad output"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn test_mark_success_idempotent(pool: PgPool) {
    setup_workspace(&pool, "ws-1").await;

    let run_id = submit_run(&pool, default_run("ws-1")).await.unwrap();

    // Mark twice — should not error (ON CONFLICT DO UPDATE)
    mark_success(&pool, run_id, "admin@test.com", Some("first"), None)
        .await
        .unwrap();
    mark_success(&pool, run_id, "admin@test.com", Some("second"), None)
        .await
        .unwrap();

    let completed = sqlx::query!(
        "SELECT mark_reason FROM run_completed WHERE id = $1",
        run_id
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(completed.mark_reason.as_deref(), Some("second"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn test_rerun_basic(pool: PgPool) {
    setup_workspace(&pool, "ws-1").await;

    // Create and finish original run
    let original_id = submit_run(&pool, default_run("ws-1")).await.unwrap();

    sqlx::query!(
        "INSERT INTO worker_ping (worker, tags) VALUES ($1, $2)",
        "worker-1",
        &["default".to_string()] as &[String]
    )
    .execute(&pool)
    .await
    .unwrap();
    claim_run(&pool, "worker-1", &["default".into()], 4.0, 4096, 10240)
        .await
        .unwrap();
    finish_run(
        &pool,
        original_id,
        false,
        serde_json::json!({"error": "failed"}),
        500,
        0,
        None,
    )
    .await
    .unwrap();

    // Rerun
    let result = rerun(&pool, original_id, "user@test.com", false)
        .await
        .unwrap();

    assert_eq!(result.original_run_id, original_id);
    assert_ne!(result.new_run_id, original_id);

    // Verify new run exists with same parameters
    let new_run = sqlx::query!(
        "SELECT workspace_id, kind, tag, script_path, rerun_of FROM run WHERE id = $1",
        result.new_run_id
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(new_run.workspace_id, "ws-1");
    assert_eq!(new_run.kind, "script");
    assert_eq!(new_run.tag, "default");
    assert_eq!(new_run.script_path.as_deref(), Some("users/test/hello"));
    assert_eq!(new_run.rerun_of, Some(original_id));

    // Verify new run is in queue
    let queue = sqlx::query!("SELECT id FROM run_queue WHERE id = $1", result.new_run_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(queue.id, result.new_run_id);
}

#[sqlx::test(migrations = "../../migrations")]
async fn test_rerun_not_found(pool: PgPool) {
    setup_workspace(&pool, "ws-1").await;

    let fake_id = uuid::Uuid::new_v4();
    let err = rerun(&pool, fake_id, "user@test.com", false)
        .await
        .unwrap_err();

    match err {
        QueueError::Other(msg) => assert!(msg.contains("not found")),
        other => panic!("expected Other, got: {other}"),
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn test_append_and_get_run_log_chunks(pool: PgPool) {
    setup_workspace(&pool, "ws-1").await;
    let run_id = submit_run(&pool, default_run("ws-1")).await.unwrap();

    let chunks = vec![
        RunLogChunk {
            run_id,
            seq: 0,
            min_level: 3,
            max_level: 3,
            line_count: 2,
            entries: serde_json::json!([
                {"ts": "2025-01-01T00:00:00Z", "level": 3, "msg": "hello"},
                {"ts": "2025-01-01T00:00:01Z", "level": 3, "msg": "world"},
            ]),
        },
        RunLogChunk {
            run_id,
            seq: 1,
            min_level: 4,
            max_level: 5,
            line_count: 1,
            entries: serde_json::json!([
                {"ts": "2025-01-01T00:00:02Z", "level": 5, "msg": "error!"},
            ]),
        },
    ];

    append_run_log_chunks(&pool, &chunks).await.unwrap();

    // Fetch all chunks
    let rows = get_run_log_chunks(&pool, run_id, 0, None, 100)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].seq, 0);
    assert_eq!(rows[0].line_count, 2);
    assert_eq!(rows[1].seq, 1);
    assert_eq!(rows[1].max_level, 5);
}

#[sqlx::test(migrations = "../../migrations")]
async fn test_run_log_chunks_cursor_pagination(pool: PgPool) {
    setup_workspace(&pool, "ws-1").await;
    let run_id = submit_run(&pool, default_run("ws-1")).await.unwrap();

    // Insert 3 chunks
    let chunks: Vec<RunLogChunk> = (0..3)
        .map(|i| RunLogChunk {
            run_id,
            seq: i,
            min_level: 3,
            max_level: 3,
            line_count: 1,
            entries: serde_json::json!([{"msg": format!("chunk {i}")}]),
        })
        .collect();

    append_run_log_chunks(&pool, &chunks).await.unwrap();

    // Get first 2
    let page1 = get_run_log_chunks(&pool, run_id, 0, None, 2).await.unwrap();
    assert_eq!(page1.len(), 2);

    // Get next page using cursor
    let cursor = page1.last().unwrap().id;
    let page2 = get_run_log_chunks(&pool, run_id, cursor, None, 2)
        .await
        .unwrap();
    assert_eq!(page2.len(), 1);
    assert_eq!(page2[0].seq, 2);
}

#[sqlx::test(migrations = "../../migrations")]
async fn test_run_log_chunks_level_filter(pool: PgPool) {
    setup_workspace(&pool, "ws-1").await;
    let run_id = submit_run(&pool, default_run("ws-1")).await.unwrap();

    let chunks = vec![
        RunLogChunk {
            run_id,
            seq: 0,
            min_level: 2,
            max_level: 3, // max is INFO
            line_count: 1,
            entries: serde_json::json!([{"msg": "debug+info chunk"}]),
        },
        RunLogChunk {
            run_id,
            seq: 1,
            min_level: 4,
            max_level: 5, // max is ERROR
            line_count: 1,
            entries: serde_json::json!([{"msg": "warn+error chunk"}]),
        },
    ];

    append_run_log_chunks(&pool, &chunks).await.unwrap();

    // Filter for WARN(4) and above — should only return the second chunk
    let rows = get_run_log_chunks(&pool, run_id, 0, Some(4), 100)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].seq, 1);

    // Filter for INFO(3) and above — should return both
    let rows = get_run_log_chunks(&pool, run_id, 0, Some(3), 100)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
}

#[sqlx::test(migrations = "../../migrations")]
async fn test_append_empty_run_log_chunks(pool: PgPool) {
    // Appending empty chunks should not error
    append_run_log_chunks(&pool, &[]).await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn test_append_and_get_service_log_chunks(pool: PgPool) {
    let chunks = vec![
        ServiceLogChunk {
            instance_id: "inst-1".to_string(),
            service: "api".to_string(),
            seq: 0,
            min_level: 3,
            max_level: 3,
            line_count: 1,
            entries: serde_json::json!([{"msg": "api started"}]),
        },
        ServiceLogChunk {
            instance_id: "inst-2".to_string(),
            service: "worker".to_string(),
            seq: 0,
            min_level: 4,
            max_level: 4,
            line_count: 1,
            entries: serde_json::json!([{"msg": "worker warning"}]),
        },
    ];

    append_service_log_chunks(&pool, &chunks).await.unwrap();

    // Fetch all
    let rows = get_service_log_chunks(&pool, None, None, 0, None, 100)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);

    // Filter by service
    let rows = get_service_log_chunks(&pool, Some("api"), None, 0, None, 100)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].service, "api");

    // Filter by instance_id
    let rows = get_service_log_chunks(&pool, None, Some("inst-2"), 0, None, 100)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].instance_id, "inst-2");
}

#[sqlx::test(migrations = "../../migrations")]
async fn test_service_log_chunks_level_filter(pool: PgPool) {
    let chunks = vec![
        ServiceLogChunk {
            instance_id: "inst-1".to_string(),
            service: "api".to_string(),
            seq: 0,
            min_level: 2,
            max_level: 2, // DEBUG only
            line_count: 1,
            entries: serde_json::json!([{"msg": "debug"}]),
        },
        ServiceLogChunk {
            instance_id: "inst-1".to_string(),
            service: "api".to_string(),
            seq: 1,
            min_level: 5,
            max_level: 5, // ERROR
            line_count: 1,
            entries: serde_json::json!([{"msg": "error"}]),
        },
    ];

    append_service_log_chunks(&pool, &chunks).await.unwrap();

    // Filter for ERROR(5) and above
    let rows = get_service_log_chunks(&pool, None, None, 0, Some(5), 100)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].seq, 1);
}

#[sqlx::test(migrations = "../../migrations")]
async fn test_service_log_chunks_cursor_pagination(pool: PgPool) {
    let chunks: Vec<ServiceLogChunk> = (0..5)
        .map(|i| ServiceLogChunk {
            instance_id: "inst-1".to_string(),
            service: "api".to_string(),
            seq: i,
            min_level: 3,
            max_level: 3,
            line_count: 1,
            entries: serde_json::json!([{"msg": format!("chunk {i}")}]),
        })
        .collect();

    append_service_log_chunks(&pool, &chunks).await.unwrap();

    let page1 = get_service_log_chunks(&pool, None, None, 0, None, 3)
        .await
        .unwrap();
    assert_eq!(page1.len(), 3);

    let cursor = page1.last().unwrap().id;
    let page2 = get_service_log_chunks(&pool, None, None, cursor, None, 3)
        .await
        .unwrap();
    assert_eq!(page2.len(), 2);
}

#[sqlx::test(migrations = "../../migrations")]
async fn test_append_empty_service_log_chunks(pool: PgPool) {
    append_service_log_chunks(&pool, &[]).await.unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn test_retention_uses_default_policy_without_workspace_settings(pool: PgPool) {
    setup_workspace(&pool, "ws-no-settings").await;
    let run_id = submit_run(&pool, default_run("ws-no-settings"))
        .await
        .unwrap();

    append_run_log_chunks(
        &pool,
        &[RunLogChunk {
            run_id,
            seq: 0,
            min_level: 3,
            max_level: 3,
            line_count: 1,
            entries: serde_json::json!([{"msg": "old log"}]),
        }],
    )
    .await
    .unwrap();

    sqlx::query!(
        "UPDATE run_log SET created_at = now() - INTERVAL '31 days' WHERE run_id = $1",
        run_id
    )
    .execute(&pool)
    .await
    .unwrap();

    let config = retention_config_without_sleep();
    let result = execute_retention(&pool, &config).await.unwrap();

    let workspace_result = result
        .workspaces
        .iter()
        .find(|workspace| workspace.workspace_id == "ws-no-settings")
        .unwrap();
    assert_eq!(workspace_result.run_log_deleted, 1);

    assert_eq!(run_log_count(&pool, run_id).await, 0);
}

#[sqlx::test(migrations = "../../migrations")]
async fn test_retention_uses_workspace_policy_boundary(pool: PgPool) {
    setup_workspace(&pool, "ws-policy").await;
    set_workspace_log_retention(&pool, "ws-policy", 10).await;

    let old_run_id = submit_run(&pool, default_run("ws-policy")).await.unwrap();
    let recent_run_id = submit_run(&pool, default_run("ws-policy")).await.unwrap();

    append_run_log_at(
        &pool,
        old_run_id,
        0,
        chrono::Utc::now() - chrono::Duration::days(11),
    )
    .await;
    append_run_log_at(
        &pool,
        recent_run_id,
        0,
        chrono::Utc::now() - chrono::Duration::days(9),
    )
    .await;

    let config = retention_config_without_sleep();
    let result = execute_retention(&pool, &config).await.unwrap();

    let workspace_result = result
        .workspaces
        .iter()
        .find(|workspace| workspace.workspace_id == "ws-policy")
        .unwrap();
    assert_eq!(workspace_result.run_log_deleted, 1);
    assert_eq!(run_log_count(&pool, old_run_id).await, 0);
    assert_eq!(run_log_count(&pool, recent_run_id).await, 1);
}

#[sqlx::test(migrations = "../../migrations")]
async fn test_retention_allows_zero_day_workspace_policy(pool: PgPool) {
    setup_workspace(&pool, "ws-zero").await;
    set_workspace_log_retention(&pool, "ws-zero", 0).await;

    let run_id = submit_run(&pool, default_run("ws-zero")).await.unwrap();
    append_run_log_at(
        &pool,
        run_id,
        0,
        chrono::Utc::now() - chrono::Duration::seconds(1),
    )
    .await;

    let config = retention_config_without_sleep();
    let result = execute_retention(&pool, &config).await.unwrap();

    let workspace_result = result
        .workspaces
        .iter()
        .find(|workspace| workspace.workspace_id == "ws-zero")
        .unwrap();
    assert_eq!(workspace_result.run_log_deleted, 1);
    assert_eq!(run_log_count(&pool, run_id).await, 0);
}

#[sqlx::test(migrations = "../../migrations")]
async fn test_retention_respects_longer_team_override(pool: PgPool) {
    setup_workspace(&pool, "ws-team-long").await;
    setup_team_quota(&pool, "ws-team-long", "ml-team", None, None, None, None).await;
    set_workspace_log_retention(&pool, "ws-team-long", 7).await;
    set_team_log_retention(&pool, "ws-team-long", "ml-team", 30).await;

    let unowned_run_id = submit_run(&pool, default_run("ws-team-long"))
        .await
        .unwrap();

    let mut team_run = default_run("ws-team-long");
    team_run.team_owner = Some("ml-team");
    let team_run_id = submit_run(&pool, team_run).await.unwrap();

    let old_log_time = chrono::Utc::now() - chrono::Duration::days(10);
    append_run_log_at(&pool, unowned_run_id, 0, old_log_time).await;
    append_run_log_at(&pool, team_run_id, 0, old_log_time).await;

    let config = retention_config_without_sleep();
    let result = execute_retention(&pool, &config).await.unwrap();

    let workspace_result = result
        .workspaces
        .iter()
        .find(|workspace| workspace.workspace_id == "ws-team-long")
        .unwrap();
    assert_eq!(workspace_result.run_log_deleted, 1);
    assert_eq!(run_log_count(&pool, unowned_run_id).await, 0);
    assert_eq!(run_log_count(&pool, team_run_id).await, 1);
}

#[sqlx::test(migrations = "../../migrations")]
async fn test_retention_respects_shorter_team_override(pool: PgPool) {
    setup_workspace(&pool, "ws-team-short").await;
    setup_team_quota(&pool, "ws-team-short", "ml-team", None, None, None, None).await;
    set_workspace_log_retention(&pool, "ws-team-short", 30).await;
    set_team_log_retention(&pool, "ws-team-short", "ml-team", 7).await;

    let unowned_run_id = submit_run(&pool, default_run("ws-team-short"))
        .await
        .unwrap();

    let mut team_run = default_run("ws-team-short");
    team_run.team_owner = Some("ml-team");
    let team_run_id = submit_run(&pool, team_run).await.unwrap();

    let old_log_time = chrono::Utc::now() - chrono::Duration::days(10);
    append_run_log_at(&pool, unowned_run_id, 0, old_log_time).await;
    append_run_log_at(&pool, team_run_id, 0, old_log_time).await;

    let config = retention_config_without_sleep();
    let result = execute_retention(&pool, &config).await.unwrap();

    let workspace_result = result
        .workspaces
        .iter()
        .find(|workspace| workspace.workspace_id == "ws-team-short")
        .unwrap();
    assert_eq!(workspace_result.run_log_deleted, 1);
    assert_eq!(run_log_count(&pool, unowned_run_id).await, 1);
    assert_eq!(run_log_count(&pool, team_run_id).await, 0);
}

#[sqlx::test(migrations = "../../migrations")]
async fn test_retention_cleans_service_logs_by_global_policy(pool: PgPool) {
    append_service_log_chunks(
        &pool,
        &[
            ServiceLogChunk {
                instance_id: "inst-1".to_string(),
                service: "worker".to_string(),
                seq: 0,
                min_level: 3,
                max_level: 3,
                line_count: 1,
                entries: serde_json::json!([{"msg": "old service log"}]),
            },
            ServiceLogChunk {
                instance_id: "inst-1".to_string(),
                service: "worker".to_string(),
                seq: 1,
                min_level: 3,
                max_level: 3,
                line_count: 1,
                entries: serde_json::json!([{"msg": "recent service log"}]),
            },
        ],
    )
    .await
    .unwrap();

    sqlx::query!(
        "UPDATE service_log SET created_at = $1 WHERE instance_id = 'inst-1' AND service = 'worker' AND seq = 0",
        chrono::Utc::now() - chrono::Duration::days(15),
    )
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query!(
        "UPDATE service_log SET created_at = $1 WHERE instance_id = 'inst-1' AND service = 'worker' AND seq = 1",
        chrono::Utc::now() - chrono::Duration::days(13),
    )
    .execute(&pool)
    .await
    .unwrap();

    let config = retention_config_without_sleep();
    let result = execute_retention(&pool, &config).await.unwrap();

    assert_eq!(result.service_log_deleted, 1);

    let remaining = sqlx::query_scalar!(
        r#"SELECT COUNT(*) as "count!" FROM service_log WHERE instance_id = 'inst-1'"#
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(remaining, 1);
}

// ---------------------------------------------------------------------------
// Liveness reaper
// ---------------------------------------------------------------------------

/// Register a worker heartbeat row so claim_run can match it.
async fn register_worker(pool: &PgPool, worker: &str) {
    sqlx::query!(
        "INSERT INTO worker_ping (worker, tags) VALUES ($1, $2)",
        worker,
        &["default".to_string()] as &[String]
    )
    .execute(pool)
    .await
    .unwrap();
}

/// Backdate a worker's heartbeat so it reads as lost to the reaper.
async fn stale_worker(pool: &PgPool, worker: &str) {
    sqlx::query!(
        "UPDATE worker_ping SET ping_at = now() - interval '10 minutes' WHERE worker = $1",
        worker
    )
    .execute(pool)
    .await
    .unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
async fn reap_fails_jobs_of_lost_worker(pool: PgPool) {
    setup_workspace(&pool, "ws-1").await;
    let run_id = submit_run(&pool, default_run("ws-1")).await.unwrap();
    register_worker(&pool, "worker-1").await;
    claim_run(&pool, "worker-1", &["default".into()], 4.0, 4096, 10240)
        .await
        .unwrap()
        .expect("should claim");

    stale_worker(&pool, "worker-1").await;

    let outcome = reap_lost_workers(&pool, 90).await.unwrap();
    assert_eq!(outcome.runs_failed, 1);
    assert_eq!(outcome.workers_removed, 1);

    // Job is off the queue and recorded as a failed completion.
    let queued = sqlx::query_scalar!(
        "SELECT count(*) AS \"n!\" FROM run_queue WHERE id = $1",
        run_id
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(queued, 0);
    let success = sqlx::query_scalar!("SELECT success FROM run_completed WHERE id = $1", run_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(!success, "lost job must be failed, not succeeded");

    // Stale worker row is gone.
    let workers = sqlx::query_scalar!("SELECT count(*) AS \"n!\" FROM worker_ping")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(workers, 0);
}

#[sqlx::test(migrations = "../../migrations")]
async fn reap_leaves_alive_worker_untouched(pool: PgPool) {
    setup_workspace(&pool, "ws-1").await;
    let run_id = submit_run(&pool, default_run("ws-1")).await.unwrap();
    register_worker(&pool, "worker-1").await;
    claim_run(&pool, "worker-1", &["default".into()], 4.0, 4096, 10240)
        .await
        .unwrap()
        .expect("should claim");

    // Worker just pinged — well within the window. Nothing should be reaped.
    let outcome = reap_lost_workers(&pool, 90).await.unwrap();
    assert_eq!(outcome, coveflow_queue::ReapOutcome::default());

    let running = sqlx::query_scalar!("SELECT running FROM run_queue WHERE id = $1", run_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(running, "alive worker's job must keep running");
    let completed = sqlx::query_scalar!(
        "SELECT count(*) AS \"n!\" FROM run_completed WHERE id = $1",
        run_id
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(completed, 0);
}

#[sqlx::test(migrations = "../../migrations")]
async fn reap_is_idempotent_when_completion_exists(pool: PgPool) {
    // Race: a recovered worker's finish_run already wrote a terminal row before
    // the reaper ran. The reaper must not collide on the run_completed PK and
    // must not overwrite the existing (success) outcome.
    setup_workspace(&pool, "ws-1").await;
    let run_id = submit_run(&pool, default_run("ws-1")).await.unwrap();
    register_worker(&pool, "worker-1").await;
    claim_run(&pool, "worker-1", &["default".into()], 4.0, 4096, 10240)
        .await
        .unwrap()
        .expect("should claim");
    stale_worker(&pool, "worker-1").await;

    // Pre-existing successful completion for this run.
    sqlx::query!(
        "INSERT INTO run_completed (id, success, result, duration_ms) VALUES ($1, TRUE, '{}'::jsonb, 5)",
        run_id
    )
    .execute(&pool)
    .await
    .unwrap();

    let outcome = reap_lost_workers(&pool, 90).await.unwrap();
    // No new failed completion written (conflict skipped), but the worker row
    // and the now-orphaned queue entry are still cleaned up.
    assert_eq!(outcome.runs_failed, 0);
    assert_eq!(outcome.workers_removed, 1);

    let success = sqlx::query_scalar!("SELECT success FROM run_completed WHERE id = $1", run_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(success, "existing successful completion must be preserved");
}

// ===========================================================================
// Flow engine (DAG)
// ===========================================================================
mod flow_engine_tests {
    use super::*;
    use coveflow_queue::{FlowProgress, NewRun, advance_flow, finish_run, submit_run};
    use coveflow_types::RunKind;
    use coveflow_types::flows::{
        Backoff, BranchCase, Expr, FlowEdge, FlowNode, FlowSpec, InputBinding, NodeBody, NodeId,
        RetryPolicy, TriggerRule,
    };
    use serde_json::{Value, json};
    use sqlx::PgPool;
    use std::collections::HashMap;
    use uuid::Uuid;

    fn node(id: &str, body: NodeBody) -> FlowNode {
        FlowNode {
            id: NodeId(id.into()),
            body,
            retry: None,
            summary: None,
            skip_if: None,
            trigger_rule: Default::default(),
            ui: None,
        }
    }
    // Deterministic script_id from a name (no uuid v5 feature needed).
    fn sid(name: &str) -> Uuid {
        let mut h: u128 = 0xcbf2_9ce4_8422_2325;
        for b in name.bytes() {
            h = (h ^ b as u128).wrapping_mul(0x0100_0000_01b3);
        }
        Uuid::from_u128(h)
    }
    fn with_rule(mut n: FlowNode, rule: TriggerRule) -> FlowNode {
        n.trigger_rule = rule;
        n
    }
    fn script_with(id: &str, script_id: Uuid, inputs: HashMap<String, InputBinding>) -> FlowNode {
        node(
            id,
            NodeBody::Script {
                script_id,
                hash: None,
                inputs,
                queue: None,
            },
        )
    }
    fn script(id: &str) -> FlowNode {
        script_with(id, sid("noop"), HashMap::new())
    }
    fn script_inputs(id: &str, inputs: &[(&str, &str)]) -> FlowNode {
        let map = inputs
            .iter()
            .map(|(k, e)| {
                (
                    k.to_string(),
                    InputBinding::Expr {
                        expr: Expr((*e).into()),
                    },
                )
            })
            .collect();
        script_with(id, sid("noop"), map)
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
    fn cond_edge(from: &str, to: &str, when: &str) -> FlowEdge {
        FlowEdge {
            from: NodeId(from.into()),
            to: NodeId(to.into()),
            when: Some(Expr(when.into())),
            case: None,
            from_handle: None,
            to_handle: None,
        }
    }
    fn spec(nodes: Vec<FlowNode>, edges: Vec<FlowEdge>) -> FlowSpec {
        FlowSpec {
            nodes,
            edges,
            on_error: None,
            max_concurrent: None,
            retry: None,
        }
    }

    // Script nodes resolve by script_id at dispatch, so the referenced scripts
    // must exist. Seed one row per referenced script_id (path is derived).
    async fn seed_script(pool: &PgPool, script_id: Uuid) {
        let hash: String = format!("{:0<64}", script_id.simple());
        let path = format!("f/test/{}", &script_id.simple().to_string()[..8]);
        sqlx::query!(
            "INSERT INTO script (workspace_id, hash, path, name, content, language, created_by, script_id)
             VALUES ('ws-1', $1, $2, $2, 'noop', 'python3', 't@test.com', $3)
             ON CONFLICT (workspace_id, hash) DO NOTHING",
            hash,
            path,
            script_id,
        )
        .execute(pool)
        .await
        .unwrap();
    }

    fn script_ids(body: &NodeBody, out: &mut Vec<Uuid>) {
        match body {
            NodeBody::Script { script_id, .. } => out.push(*script_id),
            NodeBody::Branch { task } => script_ids(task, out),
        }
    }

    fn branch(id: &str) -> FlowNode {
        node(
            id,
            NodeBody::Branch {
                task: Box::new(NodeBody::Script {
                    script_id: sid("noop"),
                    hash: None,
                    inputs: HashMap::new(),
                    queue: None,
                }),
            },
        )
    }
    fn match_edge(from: &str, to: &str, value: Value) -> FlowEdge {
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

    async fn submit_flow(pool: &PgPool, spec: &FlowSpec, input: Value) -> Uuid {
        let mut ids = Vec::new();
        for n in &spec.nodes {
            script_ids(&n.body, &mut ids);
        }
        if let Some(h) = &spec.on_error {
            script_ids(&h.body, &mut ids);
        }
        ids.sort();
        ids.dedup();
        for id in ids {
            seed_script(pool, id).await;
        }
        submit_run(
            pool,
            NewRun {
                workspace_id: "ws-1",
                kind: RunKind::Flow,
                script_hash: None,
                script_path: None,
                raw_code: None,
                language: None,
                args: Some(input),
                flow_value: Some(serde_json::to_value(spec).unwrap()),
                tag: "default",
                parent_run: None,
                root_run: None,
                flow_step_id: None,
                team_owner: None,
                created_by: "t@test.com",
                trace_id: None,
                span_id: None,
                scheduled_for: None,
                priority: None,
                cpus: None,
                memory_mb: None,
                disk_mb: None,
                requirements: vec![],
                timeout: None,
                custom_image: None,
                schedule_id: None,
                scheduled_time: None,
                data_interval_end: None,
                trigger_id: None,
                trigger_context: None,
            },
        )
        .await
        .unwrap()
    }

    /// Mini worker loop: advance flows, "execute" leaves via `leaf`.
    async fn drive<F>(pool: &PgPool, mut leaf: F) -> Vec<(String, Value)>
    where
        F: FnMut(&str, &Value) -> (bool, Value),
    {
        let mut executed = vec![];
        for _ in 0..500 {
            let row = sqlx::query!(
                r#"SELECT r.id AS "id!", r.kind AS "kind!", r.flow_step_id, r.args
                   FROM run_queue q JOIN run r ON r.id = q.id
                   WHERE q.running = FALSE AND q.scheduled_for <= now()
                     AND NOT EXISTS (SELECT 1 FROM run_completed c WHERE c.id = r.id)
                   ORDER BY q.scheduled_for ASC, r.created_at ASC LIMIT 1"#
            )
            .fetch_optional(pool)
            .await
            .unwrap();
            let Some(row) = row else { break };
            match row.kind.as_str() {
                "flow" | "flow_preview" => match advance_flow(pool, row.id).await.unwrap() {
                    FlowProgress::Suspended => {}
                    FlowProgress::Completed { result } => {
                        finish_run(pool, row.id, true, result, 0, 0, None)
                            .await
                            .unwrap();
                    }
                    FlowProgress::Failed { error } => {
                        finish_run(pool, row.id, false, error, 0, 0, None)
                            .await
                            .unwrap();
                    }
                },
                _ => {
                    let sid = row.flow_step_id.clone().unwrap_or_default();
                    let args = row.args.clone().unwrap_or(Value::Null);
                    executed.push((sid.clone(), args.clone()));
                    let (ok, result) = leaf(&sid, &args);
                    finish_run(pool, row.id, ok, result, 0, 0, None)
                        .await
                        .unwrap();
                }
            }
        }
        executed
    }

    async fn flow_outcome(pool: &PgPool, flow_id: Uuid) -> (bool, Value) {
        let row = sqlx::query!(
            "SELECT success, result FROM run_completed WHERE id = $1",
            flow_id
        )
        .fetch_one(pool)
        .await
        .unwrap();
        (row.success, row.result.unwrap_or(Value::Null))
    }

    /// Read a node's persisted state ("pending"/"running"/"succeeded"/"failed"/
    /// "skipped") from run_flow_status.
    async fn node_state(pool: &PgPool, flow_id: Uuid, node_id: &str) -> String {
        let fs: Value = sqlx::query_scalar!(
            "SELECT flow_status FROM run_flow_status WHERE run_id = $1",
            flow_id
        )
        .fetch_one(pool)
        .await
        .unwrap();
        fs["nodes"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|n| n["id"] == json!(node_id))
            .and_then(|n| n["state"].as_str().map(String::from))
            .unwrap_or_default()
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn linear_chain_passes_data(pool: PgPool) {
        setup_workspace(&pool, "ws-1").await;
        let s = spec(
            vec![
                script("a"),
                script_inputs("b", &[("count", "steps.a.result.total + 1")]),
            ],
            vec![edge("a", "b")],
        );
        let flow = submit_flow(&pool, &s, Value::Null).await;
        let executed = drive(&pool, |sid, _| match sid {
            "a" => (true, json!({"total": 41})),
            _ => (true, json!({"done": true})),
        })
        .await;
        let b = executed.iter().find(|(s, _)| s == "b").expect("b ran");
        assert_eq!(b.1["count"], 42);
        assert!(flow_outcome(&pool, flow).await.0);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn diamond_fan_out_and_in(pool: PgPool) {
        setup_workspace(&pool, "ws-1").await;
        // a -> b, a -> c, b -> d, c -> d
        let s = spec(
            vec![script("a"), script("b"), script("c"), script("d")],
            vec![
                edge("a", "b"),
                edge("a", "c"),
                edge("b", "d"),
                edge("c", "d"),
            ],
        );
        let flow = submit_flow(&pool, &s, Value::Null).await;
        let executed = drive(&pool, |_, _| (true, json!({}))).await;
        let steps: Vec<&str> = executed.iter().map(|(s, _)| s.as_str()).collect();
        for n in ["a", "b", "c", "d"] {
            assert!(steps.contains(&n), "{n} should run");
        }
        // d (fan-in) runs after both b and c.
        let di = steps.iter().position(|s| *s == "d").unwrap();
        assert!(di > steps.iter().position(|s| *s == "b").unwrap());
        assert!(di > steps.iter().position(|s| *s == "c").unwrap());
        assert!(flow_outcome(&pool, flow).await.0);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn conditional_edge_skips_untaken_branch(pool: PgPool) {
        setup_workspace(&pool, "ws-1").await;
        let s = spec(
            vec![script("a"), script("yes"), script("no")],
            vec![
                cond_edge("a", "yes", "steps.a.result.go"),
                cond_edge("a", "no", "steps.a.result.stop"),
            ],
        );
        let flow = submit_flow(&pool, &s, Value::Null).await;
        let executed = drive(&pool, |sid, _| match sid {
            "a" => (true, json!({"go": true, "stop": false})),
            _ => (true, json!({})),
        })
        .await;
        let steps: Vec<&str> = executed.iter().map(|(s, _)| s.as_str()).collect();
        assert!(steps.contains(&"yes"), "active-edge target runs");
        assert!(!steps.contains(&"no"), "inactive-edge target is skipped");
        assert!(flow_outcome(&pool, flow).await.0);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn branch_routes_to_matching_case(pool: PgPool) {
        setup_workspace(&pool, "ws-1").await;
        let s = spec(
            vec![
                branch("b"),
                script("paid"),
                script("failed"),
                script("other"),
            ],
            vec![
                match_edge("b", "paid", "paid".into()),
                match_edge("b", "failed", "failed".into()),
                default_edge("b", "other"),
            ],
        );
        let flow = submit_flow(&pool, &s, Value::Null).await;
        // The branch task ("b") returns the routing key.
        let executed = drive(&pool, |sid, _| match sid {
            "b" => (true, json!("paid")),
            _ => (true, json!({})),
        })
        .await;
        let steps: Vec<&str> = executed.iter().map(|(s, _)| s.as_str()).collect();
        assert!(steps.contains(&"paid"), "matched case runs");
        assert!(!steps.contains(&"failed"), "unmatched case skipped");
        assert!(
            !steps.contains(&"other"),
            "default skipped when a case matched"
        );
        assert_eq!(node_state(&pool, flow, "b").await, "succeeded");
        assert_eq!(node_state(&pool, flow, "failed").await, "skipped");
        assert_eq!(node_state(&pool, flow, "other").await, "skipped");
        assert!(flow_outcome(&pool, flow).await.0);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn branch_operator_retries_then_routes(pool: PgPool) {
        setup_workspace(&pool, "ws-1").await;
        // A Branch node carries a retry policy: its operator fails twice, then
        // succeeds returning the routing key. Retry applies to the operator's
        // child run just like a plain Script node.
        let mut b = branch("b");
        b.retry = Some(RetryPolicy {
            max_attempts: 2,
            backoff: Backoff::Fixed { delay_ms: 0 },
        });
        let s = spec(
            vec![b, script("paid"), script("other")],
            vec![
                match_edge("b", "paid", "paid".into()),
                default_edge("b", "other"),
            ],
        );
        let flow = submit_flow(&pool, &s, Value::Null).await;
        let mut attempts = 0;
        let executed = drive(&pool, |sid, _| {
            if sid == "b" {
                attempts += 1;
                if attempts < 3 {
                    return (false, json!({ "error": "transient" }));
                }
                return (true, json!("paid"));
            }
            (true, json!({}))
        })
        .await;
        assert_eq!(
            executed.iter().filter(|(s, _)| s == "b").count(),
            3,
            "operator retried twice"
        );
        let steps: Vec<&str> = executed.iter().map(|(s, _)| s.as_str()).collect();
        assert!(
            steps.contains(&"paid"),
            "matched case runs after the operator finally succeeds"
        );
        assert!(
            !steps.contains(&"other"),
            "default skipped when a case matched"
        );
        assert_eq!(node_state(&pool, flow, "b").await, "succeeded");
        assert!(flow_outcome(&pool, flow).await.0);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn branch_multi_target_fires_all_matches(pool: PgPool) {
        setup_workspace(&pool, "ws-1").await;
        let s = spec(
            vec![branch("b"), script("x"), script("y"), script("z")],
            vec![
                match_edge("b", "x", "a".into()),
                match_edge("b", "y", "c".into()),
                match_edge("b", "z", "d".into()),
            ],
        );
        let flow = submit_flow(&pool, &s, Value::Null).await;
        let executed = drive(&pool, |sid, _| match sid {
            "b" => (true, json!(["a", "c"])),
            _ => (true, json!({})),
        })
        .await;
        let steps: Vec<&str> = executed.iter().map(|(s, _)| s.as_str()).collect();
        assert!(
            steps.contains(&"x") && steps.contains(&"y"),
            "both matched cases run"
        );
        assert!(!steps.contains(&"z"), "unmatched case skipped");
        assert!(flow_outcome(&pool, flow).await.0);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn branch_default_runs_when_no_case_matches(pool: PgPool) {
        setup_workspace(&pool, "ws-1").await;
        let s = spec(
            vec![branch("b"), script("paid"), script("other")],
            vec![
                match_edge("b", "paid", "paid".into()),
                default_edge("b", "other"),
            ],
        );
        let flow = submit_flow(&pool, &s, Value::Null).await;
        let executed = drive(&pool, |sid, _| match sid {
            "b" => (true, json!("unknown")),
            _ => (true, json!({})),
        })
        .await;
        let steps: Vec<&str> = executed.iter().map(|(s, _)| s.as_str()).collect();
        assert!(
            steps.contains(&"other"),
            "default runs when nothing matched"
        );
        assert!(!steps.contains(&"paid"), "non-matching case skipped");
        assert!(flow_outcome(&pool, flow).await.0);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn branch_matches_number_by_value(pool: PgPool) {
        setup_workspace(&pool, "ws-1").await;
        // case authored as integer 1; operator returns float 1.0 → must still match.
        let s = spec(
            vec![branch("b"), script("x"), script("other")],
            vec![match_edge("b", "x", json!(1)), default_edge("b", "other")],
        );
        let flow = submit_flow(&pool, &s, Value::Null).await;
        let executed = drive(&pool, |sid, _| match sid {
            "b" => (true, json!(1.0)),
            _ => (true, json!({})),
        })
        .await;
        let steps: Vec<&str> = executed.iter().map(|(s, _)| s.as_str()).collect();
        assert!(
            steps.contains(&"x"),
            "1.0 matches case 1 (numeric equality)"
        );
        assert!(!steps.contains(&"other"), "default not taken");
        assert!(flow_outcome(&pool, flow).await.0);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn branch_into_join_runs_join(pool: PgPool) {
        setup_workspace(&pool, "ws-1").await;
        // b -(case x)-> x, b -(default)-> y, then x -> j, y -> j (a join). The join
        // uses none_failed_min_one_success so the skipped (non-taken) branch arm
        // doesn't skip it (all_success would).
        let s = spec(
            vec![
                branch("b"),
                script("x"),
                script("y"),
                with_rule(script("j"), TriggerRule::NoneFailedMinOneSuccess),
            ],
            vec![
                match_edge("b", "x", "x".into()),
                default_edge("b", "y"),
                edge("x", "j"),
                edge("y", "j"),
            ],
        );
        let flow = submit_flow(&pool, &s, Value::Null).await;
        let executed = drive(&pool, |sid, _| match sid {
            "b" => (true, json!("x")),
            _ => (true, json!({})),
        })
        .await;
        let steps: Vec<&str> = executed.iter().map(|(s, _)| s.as_str()).collect();
        assert!(steps.contains(&"x"), "matched case runs");
        assert!(!steps.contains(&"y"), "default skipped");
        assert!(steps.contains(&"j"), "join runs on one active upstream");
        assert_eq!(node_state(&pool, flow, "y").await, "skipped");
        assert!(flow_outcome(&pool, flow).await.0);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn branch_join_default_all_success_skips_join(pool: PgPool) {
        setup_workspace(&pool, "ws-1").await;
        // Same join, but `j` keeps the default trigger_rule (all_success): the
        // skipped (non-taken) branch arm makes the join skip too.
        let s = spec(
            vec![branch("b"), script("x"), script("y"), script("j")],
            vec![
                match_edge("b", "x", "x".into()),
                default_edge("b", "y"),
                edge("x", "j"),
                edge("y", "j"),
            ],
        );
        let flow = submit_flow(&pool, &s, Value::Null).await;
        let executed = drive(&pool, |sid, _| match sid {
            "b" => (true, json!("x")),
            _ => (true, json!({})),
        })
        .await;
        let steps: Vec<&str> = executed.iter().map(|(s, _)| s.as_str()).collect();
        assert!(
            !steps.contains(&"j"),
            "all_success join skips when an upstream skipped"
        );
        assert_eq!(node_state(&pool, flow, "j").await, "skipped");
        assert!(flow_outcome(&pool, flow).await.0, "flow still succeeds");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn all_done_cleanup_runs_after_upstream_failure(pool: PgPool) {
        setup_workspace(&pool, "ws-1").await;
        // cleanup uses all_done → runs once the upstream is terminal, even failed.
        let s = spec(
            vec![
                script("a"),
                with_rule(script("cleanup"), TriggerRule::AllDone),
            ],
            vec![edge("a", "cleanup")],
        );
        let flow = submit_flow(&pool, &s, Value::Null).await;
        let executed = drive(&pool, |sid, _| match sid {
            "a" => (false, json!({ "error": "boom" })),
            _ => (true, json!({ "cleaned": true })),
        })
        .await;
        let steps: Vec<&str> = executed.iter().map(|(s, _)| s.as_str()).collect();
        assert!(
            steps.contains(&"cleanup"),
            "all_done cleanup runs despite upstream failure"
        );
        assert_eq!(node_state(&pool, flow, "cleanup").await, "succeeded");
        assert_eq!(node_state(&pool, flow, "a").await, "failed");
        assert!(
            !flow_outcome(&pool, flow).await.0,
            "flow still fails (a failed)"
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn all_failed_handler_runs_only_on_failure(pool: PgPool) {
        setup_workspace(&pool, "ws-1").await;
        let make = || {
            spec(
                vec![script("a"), with_rule(script("h"), TriggerRule::AllFailed)],
                vec![edge("a", "h")],
            )
        };

        // a fails → the all_failed handler runs.
        let s = make();
        let flow = submit_flow(&pool, &s, Value::Null).await;
        let executed = drive(&pool, |sid, _| match sid {
            "a" => (false, json!({ "error": "x" })),
            _ => (true, json!({})),
        })
        .await;
        let steps: Vec<&str> = executed.iter().map(|(s, _)| s.as_str()).collect();
        assert!(
            steps.contains(&"h"),
            "all_failed handler runs after failure"
        );
        assert_eq!(node_state(&pool, flow, "h").await, "succeeded");
        assert!(!flow_outcome(&pool, flow).await.0);

        // a succeeds → the all_failed handler is skipped.
        let s2 = make();
        let flow2 = submit_flow(&pool, &s2, Value::Null).await;
        let executed2 = drive(&pool, |_sid, _| (true, json!({}))).await;
        let steps2: Vec<&str> = executed2.iter().map(|(s, _)| s.as_str()).collect();
        assert!(
            !steps2.contains(&"h"),
            "all_failed handler skipped when upstream succeeded"
        );
        assert_eq!(node_state(&pool, flow2, "h").await, "skipped");
        assert!(flow_outcome(&pool, flow2).await.0);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn branch_non_routable_result_fails_flow(pool: PgPool) {
        setup_workspace(&pool, "ws-1").await;
        let s = spec(
            vec![branch("b"), script("x")],
            vec![match_edge("b", "x", "go".into())],
        );
        let flow = submit_flow(&pool, &s, Value::Null).await;
        // An object is not a scalar/array-of-scalars → routing error.
        let executed = drive(&pool, |sid, _| match sid {
            "b" => (true, json!({"not": "routable"})),
            _ => (true, json!({})),
        })
        .await;
        let steps: Vec<&str> = executed.iter().map(|(s, _)| s.as_str()).collect();
        assert!(
            !steps.contains(&"x"),
            "downstream skipped on branch failure"
        );
        assert_eq!(node_state(&pool, flow, "b").await, "failed");
        assert!(!flow_outcome(&pool, flow).await.0, "flow fails");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn double_child_completion_is_idempotent(pool: PgPool) {
        setup_workspace(&pool, "ws-1").await;
        let s = spec(vec![script("a"), script("b")], vec![edge("a", "b")]);
        let flow = submit_flow(&pool, &s, Value::Null).await;

        // Advance once so root "a" is dispatched (the flow then suspends).
        advance_flow(&pool, flow).await.unwrap();
        let a_child = sqlx::query_scalar!(
            r#"SELECT id AS "id!" FROM run WHERE parent_run = $1 AND flow_step_id = 'a'"#,
            flow
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        // Complete "a" twice (e.g. recovered worker + liveness reaper both report).
        finish_run(&pool, a_child, true, json!({"ok": true}), 0, 0, None)
            .await
            .unwrap();
        finish_run(&pool, a_child, true, json!({"ok": true}), 0, 0, None)
            .await
            .unwrap();

        // Exactly one terminal row survives.
        let n = sqlx::query_scalar!(
            r#"SELECT count(*) AS "n!" FROM run_completed WHERE id = $1"#,
            a_child
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(n, 1, "double completion writes a single run_completed row");

        // The flow advances cleanly: downstream "b" runs exactly once, flow succeeds.
        let executed = drive(&pool, |_, _| (true, json!({}))).await;
        let b_runs = executed.iter().filter(|(s, _)| s == "b").count();
        assert_eq!(
            b_runs, 1,
            "downstream node runs once despite double-completion"
        );
        assert!(flow_outcome(&pool, flow).await.0);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn retries_then_succeeds(pool: PgPool) {
        setup_workspace(&pool, "ws-1").await;
        let mut a = script("a");
        a.retry = Some(RetryPolicy {
            max_attempts: 2,
            backoff: Backoff::Fixed { delay_ms: 0 },
        });
        let flow = submit_flow(&pool, &spec(vec![a], vec![]), Value::Null).await;
        let mut attempts = 0;
        let executed = drive(&pool, |sid, _| {
            if sid == "a" {
                attempts += 1;
                if attempts < 3 {
                    return (false, json!({"e": 1}));
                }
            }
            (true, json!({"ok": true}))
        })
        .await;
        assert_eq!(executed.iter().filter(|(s, _)| s == "a").count(), 3);
        assert!(flow_outcome(&pool, flow).await.0);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn flow_default_retry_applies_to_node_without_own_policy(pool: PgPool) {
        setup_workspace(&pool, "ws-1").await;
        // The node sets no retry of its own; the flow-level default kicks in.
        let mut s = spec(vec![script("a")], vec![]);
        s.retry = Some(RetryPolicy {
            max_attempts: 2,
            backoff: Backoff::Fixed { delay_ms: 0 },
        });
        let flow = submit_flow(&pool, &s, Value::Null).await;
        let mut attempts = 0;
        let executed = drive(&pool, |sid, _| {
            if sid == "a" {
                attempts += 1;
                if attempts < 3 {
                    return (false, json!({"e": 1}));
                }
            }
            (true, json!({"ok": true}))
        })
        .await;
        assert_eq!(
            executed.iter().filter(|(s, _)| s == "a").count(),
            3,
            "flow-default retry applied"
        );
        assert!(flow_outcome(&pool, flow).await.0);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn node_opts_out_of_flow_default_retry_with_zero(pool: PgPool) {
        setup_workspace(&pool, "ws-1").await;
        // Flow default would retry 5x, but the node sets max_attempts: 0 to opt out.
        let mut a = script("a");
        a.retry = Some(RetryPolicy {
            max_attempts: 0,
            backoff: Backoff::Fixed { delay_ms: 0 },
        });
        let mut s = spec(vec![a], vec![]);
        s.retry = Some(RetryPolicy {
            max_attempts: 5,
            backoff: Backoff::Fixed { delay_ms: 0 },
        });
        let flow = submit_flow(&pool, &s, Value::Null).await;
        let mut attempts = 0;
        let executed = drive(&pool, |sid, _| {
            if sid == "a" {
                attempts += 1;
            }
            (false, json!({"e": 1}))
        })
        .await;
        assert_eq!(
            executed.iter().filter(|(s, _)| s == "a").count(),
            1,
            "node opted out — ran once"
        );
        assert!(!flow_outcome(&pool, flow).await.0);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn fails_and_skips_downstream(pool: PgPool) {
        setup_workspace(&pool, "ws-1").await;
        let s = spec(vec![script("a"), script("b")], vec![edge("a", "b")]);
        let flow = submit_flow(&pool, &s, Value::Null).await;
        let executed = drive(&pool, |sid, _| match sid {
            "a" => (false, json!({"error": "boom"})),
            _ => (true, Value::Null),
        })
        .await;
        let steps: Vec<&str> = executed.iter().map(|(s, _)| s.as_str()).collect();
        assert!(!steps.contains(&"b"), "downstream of failure does not run");
        assert!(!flow_outcome(&pool, flow).await.0);
        // Downstream of a failure is persisted as Skipped, not left Pending.
        assert_eq!(node_state(&pool, flow, "a").await, "failed");
        assert_eq!(node_state(&pool, flow, "b").await, "skipped");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn node_runtime_error_runs_handler_and_skips_downstream(pool: PgPool) {
        setup_workspace(&pool, "ws-1").await;
        // b's input does string + number → runtime type error at dispatch. It must
        // fail b (not abort the whole pass), skip c, run on_error, and persist all
        // of that (no transaction rollback losing flow_status).
        let mut s = spec(
            vec![
                script("a"),
                script_inputs("b", &[("n", "steps.a.result.x + 1")]),
                script("c"),
            ],
            vec![edge("a", "b"), edge("b", "c")],
        );
        s.on_error = Some(Box::new(script("handler")));
        let flow = submit_flow(&pool, &s, Value::Null).await;
        let executed = drive(&pool, |sid, _| match sid {
            "a" => (true, json!({ "x": "not-a-number" })),
            _ => (true, json!({ "handled": true })),
        })
        .await;
        let steps: Vec<&str> = executed.iter().map(|(s, _)| s.as_str()).collect();
        assert!(
            steps.contains(&"handler"),
            "on_error handler runs despite runtime error"
        );
        assert!(
            !steps.contains(&"b"),
            "b never dispatched a child (errored at dispatch)"
        );
        assert!(!steps.contains(&"c"), "downstream c does not run");
        assert!(!flow_outcome(&pool, flow).await.0, "flow fails");
        assert_eq!(node_state(&pool, flow, "b").await, "failed");
        assert_eq!(node_state(&pool, flow, "c").await, "skipped");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn runs_error_handler_then_fails(pool: PgPool) {
        setup_workspace(&pool, "ws-1").await;
        let mut s = spec(vec![script("a")], vec![]);
        s.on_error = Some(Box::new(script("handler")));
        let flow = submit_flow(&pool, &s, Value::Null).await;
        let executed = drive(&pool, |sid, _| match sid {
            "a" => (false, json!({"error": "x"})),
            _ => (true, json!({"handled": true})),
        })
        .await;
        let steps: Vec<&str> = executed.iter().map(|(s, _)| s.as_str()).collect();
        assert!(steps.contains(&"handler"), "error handler runs");
        assert!(!flow_outcome(&pool, flow).await.0);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn error_handler_receives_failure_context(pool: PgPool) {
        setup_workspace(&pool, "ws-1").await;
        // The handler binds `flow.input.failed`, so it should receive the list of
        // failed node ids (here just "a") as its main() arg.
        let mut s = spec(vec![script("a"), script("b")], vec![edge("a", "b")]);
        s.on_error = Some(Box::new(script_inputs(
            "handler",
            &[("ids", "flow.input.failed")],
        )));
        let flow = submit_flow(&pool, &s, Value::Null).await;
        let executed = drive(&pool, |sid, _| match sid {
            "a" => (false, json!({ "oops": true })),
            _ => (true, json!({})),
        })
        .await;
        let handler_args = executed
            .iter()
            .find(|(s, _)| s == "handler")
            .map(|(_, a)| a.clone());
        assert_eq!(
            handler_args,
            Some(json!({ "ids": ["a"] })),
            "handler got failed ids"
        );
        assert!(!flow_outcome(&pool, flow).await.0);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn undispatchable_error_handler_fails_flow_without_rollback(pool: PgPool) {
        setup_workspace(&pool, "ws-1").await;
        let mut s = spec(vec![script("a"), script("b")], vec![edge("a", "b")]);
        s.on_error = Some(Box::new(node(
            "handler",
            NodeBody::Script {
                script_id: sid("gone"),
                hash: None,
                inputs: HashMap::new(),
                queue: None,
            },
        )));
        let flow = submit_flow(&pool, &s, Value::Null).await;
        // Delete the handler's script so push_handler → resolve → not found (Other).
        sqlx::query!(
            "DELETE FROM script WHERE workspace_id = 'ws-1' AND script_id = $1",
            sid("gone")
        )
        .execute(&pool)
        .await
        .unwrap();
        drive(&pool, |sid, _| match sid {
            "a" => (false, json!({ "error": "boom" })),
            _ => (true, Value::Null),
        })
        .await;
        // The pass must commit (not roll back): a Failed + b Skipped are persisted,
        // and the flow finishes failed instead of getting stuck.
        assert!(!flow_outcome(&pool, flow).await.0);
        assert_eq!(node_state(&pool, flow, "a").await, "failed");
        assert_eq!(node_state(&pool, flow, "b").await, "skipped");
    }

    // R4: the `run.*` expression namespace resolves the Airflow-style execution
    // context (interval, ds/ts in the schedule tz, schedule meta) inside a flow.
    #[sqlx::test(migrations = "../../migrations")]
    async fn run_namespace_exposes_context(pool: PgPool) {
        setup_workspace(&pool, "ws-1").await;
        // The schedule supplies the timezone + name behind run.ts / run.schedule_name.
        let schedule_id = sqlx::query_scalar!(
            "INSERT INTO schedule (workspace_id, name, flow_id, cron_expr, timezone,
                 enabled, created_by)
             VALUES ('ws-1', 'nightly', gen_random_uuid(), '0 * * * *', 'Asia/Taipei',
                 TRUE, 't@test.com')
             RETURNING id"
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        let s = spec(
            vec![script_inputs(
                "a",
                &[
                    ("ds", "run.ds"),
                    ("scheduled", "run.is_scheduled"),
                    ("start", "run.data_interval_start"),
                    ("end", "run.data_interval_end"),
                    ("sched", "run.schedule_name"),
                ],
            )],
            vec![],
        );
        seed_script(&pool, sid("noop")).await;

        let slot = "2026-01-01T20:00:00Z"
            .parse::<chrono::DateTime<chrono::Utc>>()
            .unwrap();
        let _flow = submit_run(
            &pool,
            NewRun {
                workspace_id: "ws-1",
                kind: RunKind::Flow,
                script_hash: None,
                script_path: Some("workspace/f"),
                raw_code: None,
                language: None,
                args: Some(Value::Null),
                flow_value: Some(serde_json::to_value(&s).unwrap()),
                tag: "default",
                parent_run: None,
                root_run: None,
                flow_step_id: None,
                team_owner: None,
                created_by: "t@test.com",
                trace_id: None,
                span_id: None,
                scheduled_for: None,
                priority: None,
                cpus: None,
                memory_mb: None,
                disk_mb: None,
                requirements: vec![],
                timeout: None,
                custom_image: None,
                schedule_id: Some(schedule_id),
                scheduled_time: Some(slot),
                data_interval_end: Some(
                    "2026-01-01T21:00:00Z"
                        .parse::<chrono::DateTime<chrono::Utc>>()
                        .unwrap(),
                ),
                trigger_id: None,
                trigger_context: None,
            },
        )
        .await
        .unwrap();

        let executed = drive(&pool, |_, _| (true, json!({}))).await;
        let a = &executed.iter().find(|(s, _)| s == "a").expect("a ran").1;
        // 2026-01-01T20:00Z = 2026-01-02T04:00 +08:00 in Asia/Taipei.
        assert_eq!(a["ds"], "2026-01-02");
        assert_eq!(a["scheduled"], true);
        assert_eq!(a["start"], "2026-01-01T20:00:00Z");
        assert_eq!(a["end"], "2026-01-01T21:00:00Z");
        assert_eq!(a["sched"], "nightly");
    }
}

#[cfg(test)]
mod scheduler_tests {
    use chrono::{DateTime, Duration, Utc};
    use coveflow_queue::{NewRun, build_run_context, run_due_schedules, submit_run};
    use coveflow_types::RunKind;
    use coveflow_types::ScriptLang;
    use coveflow_types::flow_status::{FlowRunState, NodeState};
    use coveflow_types::flows::NodeId;
    use sqlx::PgPool;
    use uuid::Uuid;

    fn ts(s: &str) -> DateTime<Utc> {
        s.parse().unwrap()
    }

    /// A NewRun with everything defaulted to None/empty; override fields with
    /// struct-update syntax per test.
    fn new_run(ws: &str, kind: RunKind) -> NewRun<'_> {
        NewRun {
            workspace_id: ws,
            kind,
            script_hash: None,
            script_path: None,
            raw_code: None,
            language: None,
            args: None,
            flow_value: None,
            tag: "default",
            parent_run: None,
            root_run: None,
            flow_step_id: None,
            team_owner: None,
            created_by: "u@test.local",
            trace_id: None,
            span_id: None,
            scheduled_for: None,
            priority: None,
            cpus: None,
            memory_mb: None,
            disk_mb: None,
            requirements: vec![],
            timeout: None,
            custom_image: None,
            schedule_id: None,
            scheduled_time: None,
            data_interval_end: None,
            trigger_id: None,
            trigger_context: None,
        }
    }

    async fn setup_workspace(pool: &PgPool, ws: &str) {
        sqlx::query!(
            "INSERT INTO workspace (id, name, owner) VALUES ($1, 'Test', 'test@example.com')
             ON CONFLICT DO NOTHING",
            ws
        )
        .execute(pool)
        .await
        .unwrap();
    }

    async fn seed_flow(pool: &PgPool, ws: &str, path: &str) {
        sqlx::query!(
            "INSERT INTO flow (workspace_id, path, revision, summary, value, edited_by, flow_id)
             VALUES ($1, $2, 1, '', $3, 'u@test.local', gen_random_uuid())",
            ws,
            path,
            serde_json::json!({ "nodes": [{ "id": "a", "body": { "kind": "script", "script_id": "11111111-1111-1111-1111-111111111111" } }], "edges": [] })
        )
        .execute(pool)
        .await
        .unwrap();
    }

    #[allow(clippy::too_many_arguments)]
    async fn seed_schedule(
        pool: &PgPool,
        ws: &str,
        name: &str,
        flow_path: &str,
        cron: &str,
        catchup: bool,
        max_active_runs: Option<i32>,
        next_trigger_at: chrono::DateTime<Utc>,
    ) -> Uuid {
        // Resolve the seeded flow's stable id by path; fall back to a random id so
        // tests can reference a flow that does not exist (missing-flow case).
        let flow_id = sqlx::query_scalar!(
            "SELECT flow_id FROM flow WHERE workspace_id = $1 AND path = $2
             ORDER BY revision DESC LIMIT 1",
            ws,
            flow_path,
        )
        .fetch_optional(pool)
        .await
        .unwrap()
        .unwrap_or_else(Uuid::new_v4);
        sqlx::query_scalar!(
            "INSERT INTO schedule (workspace_id, name, flow_id, cron_expr, timezone,
                 enabled, catchup, max_active_runs, next_trigger_at, created_by)
             VALUES ($1, $2, $3, $4, 'UTC', TRUE, $5, $6, $7, 'u@test.local')
             RETURNING id",
            ws,
            name,
            flow_id,
            cron,
            catchup,
            max_active_runs,
            next_trigger_at
        )
        .fetch_one(pool)
        .await
        .unwrap()
    }

    async fn runs_for_schedule(pool: &PgPool, sid: Uuid) -> i64 {
        sqlx::query_scalar!(
            r#"SELECT count(*) AS "n!" FROM run WHERE schedule_id = $1"#,
            sid
        )
        .fetch_one(pool)
        .await
        .unwrap()
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn fires_due_flow(pool: PgPool) {
        setup_workspace(&pool, "ws-1").await;
        seed_flow(&pool, "ws-1", "workspace/f").await;
        let due = Utc::now() - Duration::minutes(1);
        let sid = seed_schedule(
            &pool,
            "ws-1",
            "s",
            "workspace/f",
            "* * * * *",
            false,
            None,
            due,
        )
        .await;

        let fired = run_due_schedules(&pool, Utc::now()).await.unwrap();
        assert_eq!(fired, 1);
        assert_eq!(runs_for_schedule(&pool, sid).await, 1);

        // run is a flow run carrying the schedule_id + a flow_value snapshot.
        let row = sqlx::query!(
            "SELECT kind, flow_value, created_by FROM run WHERE schedule_id = $1",
            sid
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.kind, "flow");
        assert!(row.flow_value.is_some());
        assert_eq!(row.created_by, "u@test.local");

        // next_trigger_at advanced into the future; last_triggered_at set.
        let s = sqlx::query!(
            "SELECT next_trigger_at, last_triggered_at, last_error FROM schedule WHERE id = $1",
            sid
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(s.next_trigger_at.unwrap() > Utc::now());
        assert!(s.last_triggered_at.is_some());
        assert!(s.last_error.is_none());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn catchup_backfills_missed(pool: PgPool) {
        setup_workspace(&pool, "ws-1").await;
        seed_flow(&pool, "ws-1", "workspace/f").await;
        // 10 minutes behind, every-minute cron, catchup ON → several backfills.
        let due = Utc::now() - Duration::minutes(10);
        let sid = seed_schedule(
            &pool,
            "ws-1",
            "s",
            "workspace/f",
            "* * * * *",
            true,
            None,
            due,
        )
        .await;

        let fired = run_due_schedules(&pool, Utc::now()).await.unwrap();
        assert!(fired > 1, "catchup fired {fired}, expected several");
        assert_eq!(runs_for_schedule(&pool, sid).await, fired as i64);
        let next = sqlx::query_scalar!("SELECT next_trigger_at FROM schedule WHERE id = $1", sid)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(next.unwrap() > Utc::now());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn no_catchup_fires_once(pool: PgPool) {
        setup_workspace(&pool, "ws-1").await;
        seed_flow(&pool, "ws-1", "workspace/f").await;
        let due = Utc::now() - Duration::minutes(10);
        let sid = seed_schedule(
            &pool,
            "ws-1",
            "s",
            "workspace/f",
            "* * * * *",
            false,
            None,
            due,
        )
        .await;

        let fired = run_due_schedules(&pool, Utc::now()).await.unwrap();
        assert_eq!(
            fired, 1,
            "catchup off → exactly one fire regardless of how far behind"
        );
        assert_eq!(runs_for_schedule(&pool, sid).await, 1);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn max_active_runs_skips_when_at_capacity(pool: PgPool) {
        setup_workspace(&pool, "ws-1").await;
        seed_flow(&pool, "ws-1", "workspace/f").await;
        let due = Utc::now() - Duration::minutes(1);
        let sid = seed_schedule(
            &pool,
            "ws-1",
            "s",
            "workspace/f",
            "* * * * *",
            false,
            Some(1),
            due,
        )
        .await;

        // Pre-existing active (non-terminal) run for this schedule → at capacity.
        submit_run(
            &pool,
            NewRun {
                workspace_id: "ws-1",
                kind: RunKind::Flow,
                script_hash: None,
                script_path: Some("workspace/f"),
                raw_code: None,
                language: None,
                args: None,
                flow_value: Some(serde_json::json!({ "nodes": [], "edges": [] })),
                tag: "default",
                parent_run: None,
                root_run: None,
                flow_step_id: None,
                team_owner: None,
                created_by: "u@test.local",
                trace_id: None,
                span_id: None,
                scheduled_for: None,
                priority: None,
                cpus: None,
                memory_mb: None,
                disk_mb: None,
                requirements: vec![],
                timeout: None,
                custom_image: None,
                schedule_id: Some(sid),
                scheduled_time: None,
                data_interval_end: None,
                trigger_id: None,
                trigger_context: None,
            },
        )
        .await
        .unwrap();

        let fired = run_due_schedules(&pool, Utc::now()).await.unwrap();
        assert_eq!(fired, 0, "at capacity → no new fire");
        assert_eq!(
            runs_for_schedule(&pool, sid).await,
            1,
            "still just the pre-existing run"
        );

        let s = sqlx::query!(
            "SELECT next_trigger_at, last_error FROM schedule WHERE id = $1",
            sid
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(s.next_trigger_at.unwrap() > Utc::now(), "still advances");
        assert!(s.last_error.unwrap().contains("max_active_runs"));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn catchup_throttled_defers_unfired_slots(pool: PgPool) {
        setup_workspace(&pool, "ws-1").await;
        seed_flow(&pool, "ws-1", "workspace/f").await;
        // Far behind, every-minute cron, catchup ON, capped at 3 concurrent.
        // The submitted runs stay non-terminal, so this tick fills the cap and
        // stops. The un-fired slots must NOT be dropped: next_trigger_at parks on
        // the first un-fired slot (still in the past) so they retry next tick.
        let due = Utc::now() - Duration::minutes(10);
        let sid = seed_schedule(
            &pool,
            "ws-1",
            "s",
            "workspace/f",
            "* * * * *",
            true,
            Some(3),
            due,
        )
        .await;

        let fired = run_due_schedules(&pool, Utc::now()).await.unwrap();
        assert_eq!(fired, 3, "fires exactly max_active_runs this tick");
        assert_eq!(runs_for_schedule(&pool, sid).await, 3);

        let s = sqlx::query!(
            "SELECT next_trigger_at, last_error FROM schedule WHERE id = $1",
            sid
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        // Deferred, not skipped: next_trigger stays in the past so the remaining
        // slots are retried rather than lost past `now`.
        assert!(
            s.next_trigger_at.unwrap() <= Utc::now(),
            "un-fired catchup slots must be retried, not skipped past now"
        );
        assert!(s.last_error.unwrap().contains("throttled"));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn missing_flow_records_error_and_advances(pool: PgPool) {
        setup_workspace(&pool, "ws-1").await;
        // No flow seeded at this path.
        let due = Utc::now() - Duration::minutes(1);
        let sid = seed_schedule(
            &pool,
            "ws-1",
            "s",
            "workspace/gone",
            "* * * * *",
            false,
            None,
            due,
        )
        .await;

        let fired = run_due_schedules(&pool, Utc::now()).await.unwrap();
        assert_eq!(fired, 0);
        assert_eq!(runs_for_schedule(&pool, sid).await, 0);
        let s = sqlx::query!(
            "SELECT next_trigger_at, last_error FROM schedule WHERE id = $1",
            sid
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(s.next_trigger_at.unwrap() > Utc::now(), "not stuck");
        assert!(s.last_error.unwrap().contains("not found"));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn concurrent_ticks_fire_once(pool: PgPool) {
        setup_workspace(&pool, "ws-1").await;
        seed_flow(&pool, "ws-1", "workspace/f").await;
        let due = Utc::now() - Duration::minutes(1);
        let sid = seed_schedule(
            &pool,
            "ws-1",
            "s",
            "workspace/f",
            "* * * * *",
            false,
            None,
            due,
        )
        .await;

        // Two scheduler instances tick at the same instant. `FOR UPDATE SKIP
        // LOCKED` (plus next_trigger_at advancing on commit) must let exactly one
        // of them fire the due schedule — never both.
        let now = Utc::now();
        let (a, b) = tokio::join!(run_due_schedules(&pool, now), run_due_schedules(&pool, now));
        let total = a.unwrap() + b.unwrap();
        assert_eq!(
            total, 1,
            "concurrent ticks fired {total}, expected exactly 1"
        );
        assert_eq!(runs_for_schedule(&pool, sid).await, 1);
    }

    // --- R1: scheduler records the data interval -------------------------------

    #[sqlx::test(migrations = "../../migrations")]
    async fn scheduler_records_data_interval(pool: PgPool) {
        setup_workspace(&pool, "ws-1").await;
        seed_flow(&pool, "ws-1", "workspace/f").await;
        // Hourly schedule due at a fixed aligned slot.
        let slot = ts("2026-01-01T00:00:00Z");
        let sid = seed_schedule(
            &pool,
            "ws-1",
            "s",
            "workspace/f",
            "0 * * * *",
            false,
            None,
            slot,
        )
        .await;

        let fired = run_due_schedules(&pool, slot + Duration::minutes(1))
            .await
            .unwrap();
        assert_eq!(fired, 1);

        let row = sqlx::query!(
            "SELECT scheduled_time, data_interval_end FROM run WHERE schedule_id = $1",
            sid
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        // start = the slot, end = the next hourly slot.
        assert_eq!(row.scheduled_time.unwrap(), slot);
        assert_eq!(row.data_interval_end.unwrap(), ts("2026-01-01T01:00:00Z"));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn scheduler_records_interval_per_catchup_slot(pool: PgPool) {
        setup_workspace(&pool, "ws-1").await;
        seed_flow(&pool, "ws-1", "workspace/f").await;
        let slot = ts("2026-02-01T00:00:00Z");
        let sid = seed_schedule(
            &pool,
            "ws-1",
            "s",
            "workspace/f",
            "0 * * * *",
            true, // catchup
            None,
            slot,
        )
        .await;

        // Two hours of downtime → backfill 00:00, 01:00, 02:00.
        let fired = run_due_schedules(&pool, slot + Duration::hours(2) + Duration::minutes(1))
            .await
            .unwrap();
        assert_eq!(fired, 3);

        let rows = sqlx::query!(
            "SELECT scheduled_time, data_interval_end FROM run
             WHERE schedule_id = $1 ORDER BY scheduled_time",
            sid
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(rows.len(), 3);
        for r in &rows {
            // Each backfilled slot carries its own one-hour interval.
            assert_eq!(
                r.data_interval_end.unwrap(),
                r.scheduled_time.unwrap() + Duration::hours(1)
            );
        }
        assert_eq!(rows[0].scheduled_time.unwrap(), slot);
        assert_eq!(rows[2].scheduled_time.unwrap(), ts("2026-02-01T02:00:00Z"));
    }

    // --- R2: build_run_context -------------------------------------------------

    #[sqlx::test(migrations = "../../migrations")]
    async fn context_for_manual_run_is_zero_width(pool: PgPool) {
        setup_workspace(&pool, "ws-1").await;
        let id = submit_run(
            &pool,
            NewRun {
                script_path: Some("users/u/hello"),
                language: Some(ScriptLang::Python3),
                ..new_run("ws-1", RunKind::Script)
            },
        )
        .await
        .unwrap();

        let ctx = build_run_context(&pool, id).await.unwrap();
        assert!(!ctx.is_scheduled);
        assert!(ctx.schedule_id.is_none());
        assert_eq!(ctx.timezone, "UTC");
        // Manual run: interval collapses to a single instant at created_at.
        assert_eq!(ctx.logical_date, ctx.data_interval_start);
        assert_eq!(ctx.data_interval_start, ctx.data_interval_end);
        assert_eq!(ctx.triggered_at, ctx.logical_date);
        // No owning flow → no params, no upstream results.
        assert!(ctx.flow_input.is_none());
        assert!(ctx.flow_run_id.is_none());
        assert!(ctx.flow_path.is_none());
        assert_eq!(ctx.steps, serde_json::json!({}));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn context_for_scheduled_run_has_interval_and_schedule(pool: PgPool) {
        setup_workspace(&pool, "ws-1").await;
        seed_flow(&pool, "ws-1", "workspace/f").await;
        let slot = ts("2026-01-01T00:00:00Z");
        let sid = seed_schedule(
            &pool,
            "ws-1",
            "s",
            "workspace/f",
            "0 * * * *",
            false,
            None,
            slot,
        )
        .await;
        run_due_schedules(&pool, slot + Duration::minutes(1))
            .await
            .unwrap();

        let run_id = sqlx::query_scalar!("SELECT id FROM run WHERE schedule_id = $1", sid)
            .fetch_one(&pool)
            .await
            .unwrap();
        let ctx = build_run_context(&pool, run_id).await.unwrap();

        assert!(ctx.is_scheduled);
        assert_eq!(ctx.schedule_name.as_deref(), Some("s"));
        assert_eq!(ctx.timezone, "UTC");
        assert_eq!(ctx.logical_date, "2026-01-01T00:00:00Z");
        assert_eq!(ctx.data_interval_start, "2026-01-01T00:00:00Z");
        assert_eq!(ctx.data_interval_end, "2026-01-01T01:00:00Z");
        assert_eq!(ctx.ds, "2026-01-01");
        assert_eq!(ctx.ts, "2026-01-01T00:00:00+00:00");
        // The fired run is the flow run itself → flow_path resolves to the flow.
        assert_eq!(ctx.flow_path.as_deref(), Some("workspace/f"));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn context_ds_ts_use_schedule_timezone(pool: PgPool) {
        setup_workspace(&pool, "ws-1").await;
        seed_flow(&pool, "ws-1", "workspace/f").await;
        let slot = ts("2026-01-01T20:00:00Z"); // 04:00 next day in Asia/Taipei (+08)
        let sid = sqlx::query_scalar!(
            "INSERT INTO schedule (workspace_id, name, flow_id, cron_expr, timezone,
                 enabled, created_by, next_trigger_at)
             VALUES ($1, 's', gen_random_uuid(), '0 * * * *', 'Asia/Taipei', TRUE, 'u@test.local', $2)
             RETURNING id",
            "ws-1",
            slot,
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let run_id = submit_run(
            &pool,
            NewRun {
                kind: RunKind::Flow,
                script_path: Some("workspace/f"),
                flow_value: Some(serde_json::json!({"nodes": [], "edges": []})),
                schedule_id: Some(sid),
                scheduled_time: Some(slot),
                data_interval_end: Some(ts("2026-01-01T21:00:00Z")),
                trigger_id: None,
                trigger_context: None,
                ..new_run("ws-1", RunKind::Flow)
            },
        )
        .await
        .unwrap();

        let ctx = build_run_context(&pool, run_id).await.unwrap();
        assert_eq!(ctx.timezone, "Asia/Taipei");
        // 2026-01-01T20:00Z = 2026-01-02T04:00 +08:00.
        assert_eq!(ctx.ds, "2026-01-02");
        assert_eq!(ctx.ts, "2026-01-02T04:00:00+08:00");
        // logical_date stays canonical UTC.
        assert_eq!(ctx.logical_date, "2026-01-01T20:00:00Z");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn context_for_flow_child_inherits_root(pool: PgPool) {
        setup_workspace(&pool, "ws-1").await;
        seed_flow(&pool, "ws-1", "workspace/f").await;
        let slot = ts("2026-03-01T00:00:00Z");
        let sid = seed_schedule(
            &pool,
            "ws-1",
            "s",
            "workspace/f",
            "0 * * * *",
            false,
            None,
            slot,
        )
        .await;

        // Root flow run: carries the schedule, interval, and flow input (params).
        let root = submit_run(
            &pool,
            NewRun {
                kind: RunKind::Flow,
                script_path: Some("workspace/f"),
                flow_value: Some(serde_json::json!({"nodes": [], "edges": []})),
                args: Some(serde_json::json!({"date": "2026-03-01"})),
                schedule_id: Some(sid),
                scheduled_time: Some(slot),
                data_interval_end: Some(ts("2026-03-01T01:00:00Z")),
                trigger_id: None,
                trigger_context: None,
                ..new_run("ws-1", RunKind::Flow)
            },
        )
        .await
        .unwrap();

        // Upstream node "a" already succeeded in the flow's status.
        let mut state = FlowRunState::init([NodeId("a".into()), NodeId("b".into())]);
        state.set(NodeState::Succeeded {
            id: NodeId("a".into()),
            run_id: None,
            result: serde_json::json!({"rows": 12}),
        });
        sqlx::query!(
            "INSERT INTO run_flow_status (run_id, flow_status) VALUES ($1, $2)",
            root,
            serde_json::to_value(&state).unwrap()
        )
        .execute(&pool)
        .await
        .unwrap();

        // Child run executing node "b".
        let child = submit_run(
            &pool,
            NewRun {
                kind: RunKind::Script,
                script_path: Some("users/u/b"),
                language: Some(ScriptLang::Python3),
                parent_run: Some(root),
                root_run: Some(root),
                flow_step_id: Some("b"),
                ..new_run("ws-1", RunKind::Script)
            },
        )
        .await
        .unwrap();

        let ctx = build_run_context(&pool, child).await.unwrap();
        // Interval + schedule inherited from the root flow run.
        assert!(ctx.is_scheduled);
        assert_eq!(ctx.schedule_name.as_deref(), Some("s"));
        assert_eq!(ctx.logical_date, "2026-03-01T00:00:00Z");
        assert_eq!(ctx.data_interval_end, "2026-03-01T01:00:00Z");
        assert_eq!(ctx.flow_run_id.as_deref(), Some(root.to_string().as_str()));
        assert_eq!(ctx.flow_path.as_deref(), Some("workspace/f"));
        // Params come from the flow run's input.
        assert_eq!(
            ctx.flow_input,
            Some(serde_json::json!({"date": "2026-03-01"}))
        );
        // Steps = succeeded upstream results snapshot.
        assert_eq!(
            ctx.steps,
            serde_json::json!({ "a": { "result": { "rows": 12 } } })
        );
    }
}

// ===========================================================================
#[cfg(test)]
mod trigger_tests {
    use coveflow_queue::{TriggerError, submit_triggered_run};
    use coveflow_types::trigger::TriggerRow;
    use sqlx::PgPool;
    use uuid::Uuid;

    async fn setup_ws_flow(pool: &PgPool) -> Uuid {
        sqlx::query!(
            "INSERT INTO workspace (id, name, owner) VALUES ('ws', 'T', 'o@x.com')
             ON CONFLICT DO NOTHING"
        )
        .execute(pool)
        .await
        .unwrap();
        let flow_id = Uuid::new_v4();
        sqlx::query!(
            "INSERT INTO flow (workspace_id, path, revision, summary, value, edited_by, flow_id)
             VALUES ('ws', 'workspace/f', 1, '', $1, 'u@test', $2)",
            serde_json::json!({ "nodes": [], "edges": [] }),
            flow_id
        )
        .execute(pool)
        .await
        .unwrap();
        flow_id
    }

    async fn insert_trigger(pool: &PgPool, flow_id: Uuid, config: serde_json::Value) -> TriggerRow {
        let id = sqlx::query_scalar!(
            "INSERT INTO trigger (workspace_id, flow_id, trigger_type, name, config, created_by)
             VALUES ('ws', $1, 'webhook', 'hook', $2, 'creator@x.com') RETURNING id",
            flow_id,
            config
        )
        .fetch_one(pool)
        .await
        .unwrap();
        TriggerRow {
            id,
            workspace_id: "ws".into(),
            flow_id,
            trigger_type: "webhook".into(),
            name: "hook".into(),
            enabled: true,
            config,
            created_by: "creator@x.com".into(),
        }
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn fires_run_linked_to_trigger(pool: PgPool) {
        let flow_id = setup_ws_flow(&pool).await;
        let trig = insert_trigger(&pool, flow_id, serde_json::json!({})).await;

        let run_id = submit_triggered_run(
            &pool,
            &trig,
            "caller@x.com",
            serde_json::json!({ "x": 1 }),
            serde_json::json!({ "source_ip": "1.2.3.4" }),
        )
        .await
        .unwrap();

        let row = sqlx::query!(
            "SELECT created_by, trigger_id, args, trigger_context, kind FROM run WHERE id = $1",
            run_id
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        // Runs as the caller, linked to the trigger, with body as input + provenance.
        assert_eq!(row.created_by, "caller@x.com");
        assert_eq!(row.trigger_id, Some(trig.id));
        assert_eq!(row.kind, "flow");
        assert_eq!(row.args, Some(serde_json::json!({ "x": 1 })));
        assert_eq!(
            row.trigger_context,
            Some(serde_json::json!({ "source_ip": "1.2.3.4" }))
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn max_active_runs_blocks_overflow(pool: PgPool) {
        let flow_id = setup_ws_flow(&pool).await;
        let trig =
            insert_trigger(&pool, flow_id, serde_json::json!({ "max_active_runs": 1 })).await;

        // First fire succeeds; it stays non-terminal (no worker), so the second is blocked.
        submit_triggered_run(
            &pool,
            &trig,
            "c@x.com",
            serde_json::json!({}),
            serde_json::json!({}),
        )
        .await
        .unwrap();
        let err = submit_triggered_run(
            &pool,
            &trig,
            "c@x.com",
            serde_json::json!({}),
            serde_json::json!({}),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, TriggerError::MaxActiveRuns(_)));
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn triggered_run_exposes_ctx_trigger(pool: PgPool) {
        let flow_id = setup_ws_flow(&pool).await;
        let trig = insert_trigger(&pool, flow_id, serde_json::json!({})).await;
        let provenance = serde_json::json!({ "type": "webhook", "source_ip": "9.9.9.9" });
        let run_id = submit_triggered_run(
            &pool,
            &trig,
            "c@x.com",
            serde_json::json!({}),
            provenance.clone(),
        )
        .await
        .unwrap();

        let ctx = coveflow_queue::build_run_context(&pool, run_id)
            .await
            .unwrap();
        assert_eq!(ctx.trigger, Some(provenance));
    }
}
