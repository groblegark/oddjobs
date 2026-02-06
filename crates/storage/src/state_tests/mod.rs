// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

mod action_attempts;
mod agents;
mod cron;
mod decisions;
mod idempotency;
mod queue;
mod step_history;
mod workers;

use super::*;
pub(super) use oj_core::test_support::{
    job_create_event, job_delete_event, job_transition_event, queue_failed_event,
    queue_pushed_event, queue_taken_event, session_create_event, session_delete_event,
    step_failed_event, step_started_event, worker_start_event,
};
use oj_core::{AgentRunId, Event, JobId, OwnerId, SessionId, StepOutcome, WorkspaceId};

// ── Basic job CRUD ───────────────────────────────────────────────────────────

#[test]
fn apply_event_job_create() {
    let mut state = MaterializedState::default();
    state.apply_event(&job_create_event("pipe-1", "build", "test", "init"));

    assert!(state.jobs.contains_key("pipe-1"));
}

#[test]
fn apply_event_job_delete() {
    let mut state = MaterializedState::default();
    state.apply_event(&job_create_event("pipe-1", "build", "test", "init"));
    state.apply_event(&job_delete_event("pipe-1"));

    assert!(!state.jobs.contains_key("pipe-1"));
}

#[test]
fn apply_event_job_transition() {
    let mut state = MaterializedState::default();
    state.apply_event(&job_create_event("pipe-1", "build", "test", "init"));

    assert_eq!(state.jobs["pipe-1"].step, "init");

    state.apply_event(&job_transition_event("pipe-1", "build"));

    assert_eq!(state.jobs["pipe-1"].step, "build");
    assert_eq!(
        state.jobs["pipe-1"].step_status,
        oj_core::StepStatus::Pending
    );
}

#[test]
fn apply_event_step_started() {
    let mut state = MaterializedState::default();
    state.apply_event(&job_create_event("pipe-1", "build", "test", "init"));

    state.apply_event(&step_started_event("pipe-1"));

    assert_eq!(
        state.jobs["pipe-1"].step_status,
        oj_core::StepStatus::Running
    );
}

#[test]
fn apply_event_step_waiting_with_reason_sets_job_error() {
    let mut state = MaterializedState::default();
    state.apply_event(&job_create_event("pipe-1", "build", "test", "init"));

    assert!(state.jobs["pipe-1"].error.is_none());

    state.apply_event(&Event::StepWaiting {
        job_id: JobId::new("pipe-1"),
        step: "init".to_string(),
        reason: Some("gate `make test` failed (exit 1): tests failed".to_string()),
        decision_id: None,
    });

    assert!(state.jobs["pipe-1"].step_status.is_waiting());
    assert_eq!(
        state.jobs["pipe-1"].error.as_deref(),
        Some("gate `make test` failed (exit 1): tests failed")
    );
}

#[test]
fn apply_event_step_started_preserves_existing_error() {
    let mut state = MaterializedState::default();
    state.apply_event(&job_create_event("pipe-1", "build", "test", "init"));

    state.apply_event(&Event::StepWaiting {
        job_id: JobId::new("pipe-1"),
        step: "init".to_string(),
        reason: Some("previous error".to_string()),
        decision_id: None,
    });

    // StepStarted should not clear existing error
    state.apply_event(&step_started_event("pipe-1"));

    assert_eq!(
        state.jobs["pipe-1"].error.as_deref(),
        Some("previous error")
    );
}

#[test]
fn cancelled_job_is_terminal_after_event_replay() {
    let mut state = MaterializedState::default();
    state.apply_event(&job_create_event("pipe-1", "build", "test", "execute"));
    state.apply_event(&step_started_event("pipe-1"));

    state.apply_event(&job_transition_event("pipe-1", "cancelled"));
    state.apply_event(&step_failed_event("pipe-1", "execute", "cancelled"));

    let job = &state.jobs["pipe-1"];
    assert!(job.is_terminal());
    assert_eq!(job.step, "cancelled");
    assert_eq!(job.step_status, oj_core::StepStatus::Failed);
    assert_eq!(job.error.as_deref(), Some("cancelled"));
}

// ── Workspace lifecycle ──────────────────────────────────────────────────────

#[test]
fn apply_event_workspace_lifecycle() {
    let mut state = MaterializedState::default();
    state.apply_event(&Event::WorkspaceCreated {
        id: WorkspaceId::new("ws-1"),
        path: PathBuf::from("/tmp/test"),
        branch: Some("feature/test".to_string()),
        owner: Some(OwnerId::Job(JobId::new("pipe-1"))),
        workspace_type: None,
    });

    assert!(state.workspaces.contains_key("ws-1"));
    assert_eq!(state.workspaces["ws-1"].path, PathBuf::from("/tmp/test"));
    assert_eq!(
        state.workspaces["ws-1"].branch,
        Some("feature/test".to_string())
    );
    assert_eq!(
        state.workspaces["ws-1"].owner,
        Some(OwnerId::Job(JobId::new("pipe-1")))
    );
    assert_eq!(
        state.workspaces["ws-1"].status,
        oj_core::WorkspaceStatus::Creating
    );

    state.apply_event(&Event::WorkspaceReady {
        id: WorkspaceId::new("ws-1"),
    });
    assert_eq!(
        state.workspaces["ws-1"].status,
        oj_core::WorkspaceStatus::Ready
    );

    state.apply_event(&Event::WorkspaceDeleted {
        id: WorkspaceId::new("ws-1"),
    });
    assert!(!state.workspaces.contains_key("ws-1"));
}

#[yare::parameterized(
    folder_explicit   = { Some("folder"),   WorkspaceType::Folder },
    worktree_explicit = { Some("worktree"), WorkspaceType::Worktree },
    none_defaults     = { None,             WorkspaceType::Folder },
)]
fn workspace_type(ws_type: Option<&str>, expected: WorkspaceType) {
    let mut state = MaterializedState::default();
    state.apply_event(&Event::WorkspaceCreated {
        id: WorkspaceId::new("ws-1"),
        path: PathBuf::from("/tmp/ws"),
        branch: None,
        owner: None,
        workspace_type: ws_type.map(String::from),
    });

    assert_eq!(state.workspaces["ws-1"].workspace_type, expected);
}

// ── get_job prefix lookup ────────────────────────────────────────────────────

#[test]
fn get_job_exact_match() {
    let mut state = MaterializedState::default();
    state.apply_event(&job_create_event("pipe-abc123", "build", "test", "init"));

    assert!(state.get_job("pipe-abc123").is_some());
    assert_eq!(state.get_job("pipe-abc123").unwrap().id, "pipe-abc123");
}

#[test]
fn get_job_prefix_match() {
    let mut state = MaterializedState::default();
    state.apply_event(&job_create_event("pipe-abc123", "build", "test", "init"));

    assert!(state.get_job("pipe-abc").is_some());
    assert_eq!(state.get_job("pipe-abc").unwrap().id, "pipe-abc123");
}

#[test]
fn get_job_ambiguous_prefix() {
    let mut state = MaterializedState::default();
    state.apply_event(&job_create_event("pipe-abc123", "build", "test1", "init"));
    state.apply_event(&job_create_event("pipe-abc456", "build", "test2", "init"));

    // "pipe-abc" matches both, so returns None
    assert!(state.get_job("pipe-abc").is_none());
}

#[test]
fn get_job_no_match() {
    let mut state = MaterializedState::default();
    state.apply_event(&job_create_event("pipe-abc123", "build", "test", "init"));

    assert!(state.get_job("pipe-xyz").is_none());
}

// ── Session lifecycle ────────────────────────────────────────────────────────

#[test]
fn apply_event_session_lifecycle() {
    let mut state = MaterializedState::default();
    state.apply_event(&session_create_event("sess-1", "pipe-1"));

    assert!(state.sessions.contains_key("sess-1"));
    assert_eq!(state.sessions["sess-1"].job_id, "pipe-1");

    state.apply_event(&session_delete_event("sess-1"));

    assert!(!state.sessions.contains_key("sess-1"));
}

#[test]
fn session_created_with_agent_run_id_sets_session_on_agent_run() {
    let mut state = MaterializedState::default();

    let ar_id = oj_core::AgentRunId::new("ar-1");
    state.apply_event(&Event::AgentRunCreated {
        id: ar_id.clone(),
        agent_name: "worker".to_string(),
        command_name: "fix".to_string(),
        namespace: String::new(),
        cwd: PathBuf::from("/test"),
        runbook_hash: "abc".to_string(),
        vars: [("description".to_string(), "fix the bug".to_string())]
            .into_iter()
            .collect(),
        created_at_epoch_ms: 1_000_000,
    });

    assert!(state.agent_runs.contains_key("ar-1"));
    assert!(state.agent_runs["ar-1"].session_id.is_none());

    state.apply_event(&Event::SessionCreated {
        id: SessionId::new("sess-1"),
        owner: OwnerId::AgentRun(ar_id),
    });

    assert!(state.sessions.contains_key("sess-1"));
    assert_eq!(
        state.agent_runs["ar-1"].session_id.as_deref(),
        Some("sess-1")
    );
}
