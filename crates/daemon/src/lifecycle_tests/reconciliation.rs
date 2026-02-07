// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use super::*;

#[tokio::test]
async fn reconcile_state_resumes_running_workers() {
    // Workers with status "running" should be re-emitted as WorkerStarted
    // events during reconciliation so the runtime recreates their in-memory
    // state and triggers an initial queue poll.
    let dir = tempdir().unwrap();
    let dir_path = dir.path().to_owned();

    let session_adapter = TracedSession::new(TmuxAdapter::new());
    let agent_adapter = TracedAgent::new(ClaudeAgentAdapter::new(session_adapter.clone()));
    let (internal_tx, _internal_rx) = mpsc::channel::<Event>(100);

    let state = Arc::new(Mutex::new(MaterializedState::default()));
    let runtime = Arc::new(Runtime::new(
        RuntimeDeps {
            sessions: session_adapter.clone(),
            agents: agent_adapter,
            notifier: DesktopNotifyAdapter::new(),
            state: Arc::clone(&state),
        },
        SystemClock,
        RuntimeConfig {
            state_dir: dir_path.clone(),
            log_dir: dir_path.join("logs"),
        },
        internal_tx,
    ));

    // Build state with a running worker and a stopped worker
    let mut test_state = MaterializedState::default();
    test_state.workers.insert(
        "myns/running-worker".to_string(),
        WorkerRecord {
            name: "running-worker".to_string(),
            namespace: "myns".to_string(),
            project_root: dir_path.clone(),
            runbook_hash: "abc123".to_string(),
            status: "running".to_string(),
            active_job_ids: vec![],
            queue_name: "tasks".to_string(),
            concurrency: 2,
        },
    );
    test_state.workers.insert(
        "myns/stopped-worker".to_string(),
        WorkerRecord {
            name: "stopped-worker".to_string(),
            namespace: "myns".to_string(),
            project_root: dir_path.clone(),
            runbook_hash: "def456".to_string(),
            status: "stopped".to_string(),
            active_job_ids: vec![],
            queue_name: "other".to_string(),
            concurrency: 1,
        },
    );

    let events = run_reconcile(&runtime, &session_adapter, test_state).await;

    // Should have emitted exactly one WorkerStarted for the running worker
    let worker_started_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, Event::WorkerStarted { .. }))
        .collect();
    assert_eq!(
        worker_started_events.len(),
        1,
        "should emit WorkerStarted for the one running worker, got: {:?}",
        worker_started_events
    );

    // Verify the event has the right fields
    match &worker_started_events[0] {
        Event::WorkerStarted {
            worker_name,
            project_root,
            runbook_hash,
            queue_name,
            concurrency,
            namespace,
        } => {
            assert_eq!(worker_name, "running-worker");
            assert_eq!(*project_root, dir_path);
            assert_eq!(runbook_hash, "abc123");
            assert_eq!(queue_name, "tasks");
            assert_eq!(*concurrency, 2);
            assert_eq!(namespace, "myns");
        }
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn reconcile_job_dead_session_uses_step_history_agent_id() {
    // When a job's tmux session is dead, reconciliation should emit
    // AgentGone with the agent_id from step_history (a UUID), not a
    // fabricated "{job_id}-{step}" string.
    let dir = tempdir().unwrap();
    let dir_path = dir.path().to_owned();
    let (runtime, session_adapter) = setup_reconcile_runtime(&dir_path);

    let agent_uuid = "a1b2c3d4-e5f6-7890-abcd-ef1234567890";
    let mut test_state = MaterializedState::default();
    test_state.jobs.insert(
        "pipe-1".to_string(),
        make_job_with_agent("pipe-1", "build", agent_uuid, "oj-nonexistent-session"),
    );

    let events = run_reconcile(&runtime, &session_adapter, test_state).await;

    // Should emit AgentGone with the UUID from step_history
    let gone_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, Event::AgentGone { .. }))
        .collect();
    assert_eq!(
        gone_events.len(),
        1,
        "should emit exactly one AgentGone event"
    );

    match &gone_events[0] {
        Event::AgentGone { agent_id, .. } => {
            assert_eq!(
                agent_id.as_str(),
                agent_uuid,
                "AgentGone must use UUID from step_history, not job_id-step"
            );
        }
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn reconcile_job_no_agent_id_in_step_history_emits_job_failed() {
    // When a job has no agent_id in step_history (e.g., shell step
    // or crashed before agent was recorded), reconciliation should
    // emit JobAdvanced{step:"failed"} to terminate the zombie job.
    let dir = tempdir().unwrap();
    let dir_path = dir.path().to_owned();
    let (runtime, session_adapter) = setup_reconcile_runtime(&dir_path);

    let mut job = make_job_with_agent("pipe-2", "work", "any", "oj-nonexistent");
    // Clear agent_id from step_history
    job.step_history[0].agent_id = None;

    let mut test_state = MaterializedState::default();
    test_state.jobs.insert("pipe-2".to_string(), job);

    let events = run_reconcile(&runtime, &session_adapter, test_state).await;

    // Should emit JobAdvanced{step:"failed"} instead of agent events
    let advanced_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, Event::JobAdvanced { .. }))
        .collect();
    assert_eq!(
        advanced_events.len(),
        1,
        "should emit exactly one JobAdvanced event, got: {:?}",
        advanced_events
    );
    match &advanced_events[0] {
        Event::JobAdvanced { id, step } => {
            assert_eq!(id, &JobId::new("pipe-2"));
            assert_eq!(step, "failed");
        }
        _ => unreachable!(),
    }

    // Should not emit any agent events for this job
    let agent_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, Event::AgentGone { .. } | Event::AgentExited { .. }))
        .collect();
    assert!(
        agent_events.is_empty(),
        "should not emit agent events when step_history has no agent_id, got: {:?}",
        agent_events
    );
}

#[tokio::test]
async fn reconcile_job_no_session_id_emits_job_failed() {
    // When a job has no session_id (daemon crashed before session was
    // created), reconciliation should emit JobAdvanced{step:"failed"}.
    let dir = tempdir().unwrap();
    let dir_path = dir.path().to_owned();
    let (runtime, session_adapter) = setup_reconcile_runtime(&dir_path);

    // Job with no session_id (builder default is None)
    let job = Job::builder().id("pipe-nosess").step("work").build();

    let mut test_state = MaterializedState::default();
    test_state.jobs.insert("pipe-nosess".to_string(), job);

    let events = run_reconcile(&runtime, &session_adapter, test_state).await;

    let advanced_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, Event::JobAdvanced { .. }))
        .collect();
    assert_eq!(
        advanced_events.len(),
        1,
        "should emit exactly one JobAdvanced event"
    );
    match &advanced_events[0] {
        Event::JobAdvanced { id, step } => {
            assert_eq!(id, &JobId::new("pipe-nosess"));
            assert_eq!(step, "failed");
        }
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn reconcile_job_no_session_id_waiting_is_skipped() {
    // A Waiting job with no session_id should be skipped (already escalated).
    let dir = tempdir().unwrap();
    let dir_path = dir.path().to_owned();
    let (runtime, session_adapter) = setup_reconcile_runtime(&dir_path);

    let job = Job::builder()
        .id("pipe-waiting")
        .step("work")
        .step_status(StepStatus::Waiting(Some("escalated".to_string())))
        .build();

    let mut test_state = MaterializedState::default();
    test_state.jobs.insert("pipe-waiting".to_string(), job);

    let events = run_reconcile(&runtime, &session_adapter, test_state).await;

    // Should not emit any events for a Waiting job
    let job_events: Vec<_> = events
        .iter()
        .filter(|e| {
            matches!(
                e,
                Event::JobAdvanced { .. } | Event::AgentGone { .. } | Event::AgentExited { .. }
            )
        })
        .collect();
    assert!(
        job_events.is_empty(),
        "should not emit events for Waiting job, got: {:?}",
        job_events
    );
}

#[tokio::test]
async fn reconcile_agent_run_dead_session_emits_gone_with_correct_id() {
    // When an agent run's tmux session is dead, reconciliation should
    // emit AgentGone with the agent_id from the agent_run record.
    let dir = tempdir().unwrap();
    let dir_path = dir.path().to_owned();
    let (runtime, session_adapter) = setup_reconcile_runtime(&dir_path);

    let agent_uuid = "deadbeef-1234-5678-9abc-def012345678";
    let mut test_state = MaterializedState::default();
    test_state.agent_runs.insert(
        "ar-1".to_string(),
        AgentRun {
            id: "ar-1".to_string(),
            agent_name: "test-agent".to_string(),
            command_name: "do-work".to_string(),
            namespace: "proj".to_string(),
            cwd: dir_path.clone(),
            runbook_hash: "hash123".to_string(),
            status: AgentRunStatus::Running,
            agent_id: Some(agent_uuid.to_string()),
            session_id: Some("oj-nonexistent-ar-session".to_string()),
            error: None,
            created_at_ms: 1000,
            updated_at_ms: 2000,
            action_tracker: Default::default(),
            vars: HashMap::new(),
            idle_grace_log_size: None,
            last_nudge_at: None,
        },
    );

    let events = run_reconcile(&runtime, &session_adapter, test_state).await;

    // Should emit AgentGone with the correct UUID
    let gone_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, Event::AgentGone { .. }))
        .collect();
    assert_eq!(
        gone_events.len(),
        1,
        "should emit exactly one AgentGone event"
    );
    match &gone_events[0] {
        Event::AgentGone { agent_id, .. } => {
            assert_eq!(agent_id.as_str(), agent_uuid);
        }
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn reconcile_agent_run_no_agent_id_marks_failed_directly() {
    // When an agent run has no agent_id (daemon crashed before
    // AgentRunStarted was persisted), reconciliation should directly
    // emit AgentRunStatusChanged(Failed) instead of trying to route
    // through AgentExited/AgentGone events that would be dropped.
    let dir = tempdir().unwrap();
    let dir_path = dir.path().to_owned();
    let (runtime, session_adapter) = setup_reconcile_runtime(&dir_path);

    let mut test_state = MaterializedState::default();
    test_state.agent_runs.insert(
        "ar-2".to_string(),
        AgentRun {
            id: "ar-2".to_string(),
            agent_name: "test-agent".to_string(),
            command_name: "do-work".to_string(),
            namespace: "proj".to_string(),
            cwd: dir_path.clone(),
            runbook_hash: "hash123".to_string(),
            status: AgentRunStatus::Starting,
            agent_id: None, // No agent_id yet
            session_id: Some("oj-nonexistent-ar-session".to_string()),
            error: None,
            created_at_ms: 1000,
            updated_at_ms: 2000,
            action_tracker: Default::default(),
            vars: HashMap::new(),
            idle_grace_log_size: None,
            last_nudge_at: None,
        },
    );

    let events = run_reconcile(&runtime, &session_adapter, test_state).await;

    // Should emit AgentRunStatusChanged(Failed) directly
    let status_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, Event::AgentRunStatusChanged { .. }))
        .collect();
    assert_eq!(
        status_events.len(),
        1,
        "should emit exactly one AgentRunStatusChanged event"
    );
    match &status_events[0] {
        Event::AgentRunStatusChanged { id, status, reason } => {
            assert_eq!(id, &AgentRunId::new("ar-2"));
            assert_eq!(*status, AgentRunStatus::Failed);
            assert!(
                reason.as_ref().unwrap().contains("no agent_id"),
                "reason should mention missing agent_id, got: {:?}",
                reason
            );
        }
        _ => unreachable!(),
    }

    // Should NOT emit AgentGone or AgentExited
    let agent_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, Event::AgentGone { .. } | Event::AgentExited { .. }))
        .collect();
    assert!(
        agent_events.is_empty(),
        "should not emit AgentGone/AgentExited when agent_id is None"
    );
}

#[tokio::test]
async fn reconcile_agent_run_no_session_id_marks_failed() {
    // When an agent run has no session_id, reconciliation should
    // directly emit AgentRunStatusChanged(Failed).
    let dir = tempdir().unwrap();
    let dir_path = dir.path().to_owned();
    let (runtime, session_adapter) = setup_reconcile_runtime(&dir_path);

    let mut test_state = MaterializedState::default();
    test_state.agent_runs.insert(
        "ar-3".to_string(),
        AgentRun {
            id: "ar-3".to_string(),
            agent_name: "test-agent".to_string(),
            command_name: "do-work".to_string(),
            namespace: "proj".to_string(),
            cwd: dir_path.clone(),
            runbook_hash: "hash123".to_string(),
            status: AgentRunStatus::Running,
            agent_id: Some("some-uuid".to_string()),
            session_id: None, // No session
            error: None,
            created_at_ms: 1000,
            updated_at_ms: 2000,
            action_tracker: Default::default(),
            vars: HashMap::new(),
            idle_grace_log_size: None,
            last_nudge_at: None,
        },
    );

    let events = run_reconcile(&runtime, &session_adapter, test_state).await;

    let status_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, Event::AgentRunStatusChanged { .. }))
        .collect();
    assert_eq!(status_events.len(), 1);
    match &status_events[0] {
        Event::AgentRunStatusChanged { id, status, reason } => {
            assert_eq!(id, &AgentRunId::new("ar-3"));
            assert_eq!(*status, AgentRunStatus::Failed);
            assert!(reason.as_ref().unwrap().contains("no session"));
        }
        _ => unreachable!(),
    }
}
