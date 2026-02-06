// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use super::*;

fn step_started_with_agent(job_id: &str, agent_id: &str) -> Event {
    Event::StepStarted {
        job_id: JobId::new(job_id),
        step: "init".to_string(),
        agent_id: Some(oj_core::AgentId::new(agent_id)),
        agent_name: Some("worker".to_string()),
    }
}

fn state_with_job_agent(job_id: &str, agent_id: &str) -> MaterializedState {
    let mut state = MaterializedState::default();
    state.apply_event(&job_create_event(job_id, "build", "test", "init"));
    state.apply_event(&step_started_with_agent(job_id, agent_id));
    state
}

#[test]
fn populated_from_step_started_with_agent() {
    let state = state_with_job_agent("pipe-1", "agent-abc");

    assert!(state.agents.contains_key("agent-abc"));
    let record = &state.agents["agent-abc"];
    assert_eq!(record.agent_id, "agent-abc");
    assert_eq!(record.agent_name, "worker");
    assert_eq!(record.owner, OwnerId::Job(JobId::new("pipe-1")));
    assert_eq!(record.status, oj_core::AgentRecordStatus::Starting);
}

#[test]
fn not_populated_from_step_started_without_agent() {
    let mut state = MaterializedState::default();
    state.apply_event(&job_create_event("pipe-1", "build", "test", "init"));
    state.apply_event(&step_started_event("pipe-1")); // no agent_id

    assert!(state.agents.is_empty());
}

#[test]
fn populated_from_agent_run_started() {
    let mut state = MaterializedState::default();
    let ar_id = AgentRunId::new("ar-1");

    state.apply_event(&Event::AgentRunCreated {
        id: ar_id.clone(),
        agent_name: "fixer".to_string(),
        command_name: "fix".to_string(),
        namespace: "myproj".to_string(),
        cwd: PathBuf::from("/work/dir"),
        runbook_hash: "abc".to_string(),
        vars: HashMap::new(),
        created_at_epoch_ms: 1_000,
    });

    state.apply_event(&Event::AgentRunStarted {
        id: ar_id.clone(),
        agent_id: oj_core::AgentId::new("agent-xyz"),
    });

    assert!(state.agents.contains_key("agent-xyz"));
    let record = &state.agents["agent-xyz"];
    assert_eq!(record.agent_name, "fixer");
    assert_eq!(record.owner, OwnerId::AgentRun(ar_id));
    assert_eq!(record.namespace, "myproj");
    assert_eq!(record.workspace_path, PathBuf::from("/work/dir"));
    assert_eq!(record.status, oj_core::AgentRecordStatus::Running);
}

#[test]
fn status_updates_from_lifecycle_events() {
    let state = state_with_job_agent("pipe-1", "agent-1");
    assert_eq!(
        state.agents["agent-1"].status,
        oj_core::AgentRecordStatus::Starting
    );

    // Working → Idle → Exited
    let mut state = state;
    state.apply_event(&Event::AgentWorking {
        agent_id: oj_core::AgentId::new("agent-1"),
        owner: OwnerId::Job(JobId::new("pipe-1")),
    });
    assert_eq!(
        state.agents["agent-1"].status,
        oj_core::AgentRecordStatus::Running
    );

    state.apply_event(&Event::AgentWaiting {
        agent_id: oj_core::AgentId::new("agent-1"),
        owner: OwnerId::Job(JobId::new("pipe-1")),
    });
    assert_eq!(
        state.agents["agent-1"].status,
        oj_core::AgentRecordStatus::Idle
    );

    state.apply_event(&Event::AgentExited {
        agent_id: oj_core::AgentId::new("agent-1"),
        exit_code: Some(0),
        owner: OwnerId::Job(JobId::new("pipe-1")),
    });
    assert_eq!(
        state.agents["agent-1"].status,
        oj_core::AgentRecordStatus::Exited
    );
}

#[test]
fn gone_status() {
    let mut state = state_with_job_agent("pipe-1", "agent-1");

    state.apply_event(&Event::AgentGone {
        agent_id: oj_core::AgentId::new("agent-1"),
        owner: OwnerId::Job(JobId::new("pipe-1")),
    });
    assert_eq!(
        state.agents["agent-1"].status,
        oj_core::AgentRecordStatus::Gone
    );
}

#[test]
fn session_id_set_by_session_created() {
    let mut state = state_with_job_agent("pipe-1", "agent-1");
    assert!(state.agents["agent-1"].session_id.is_none());

    state.apply_event(&session_create_event("sess-1", "pipe-1"));

    assert_eq!(
        state.agents["agent-1"].session_id.as_deref(),
        Some("sess-1")
    );
}

#[test]
fn session_id_cleared_by_session_deleted() {
    let mut state = state_with_job_agent("pipe-1", "agent-1");
    state.apply_event(&session_create_event("sess-1", "pipe-1"));
    state.apply_event(&session_delete_event("sess-1"));

    assert!(state.agents["agent-1"].session_id.is_none());
}

#[test]
fn removed_on_job_deleted() {
    let mut state = state_with_job_agent("pipe-1", "agent-1");
    assert!(state.agents.contains_key("agent-1"));

    state.apply_event(&job_delete_event("pipe-1"));

    assert!(!state.agents.contains_key("agent-1"));
}

#[test]
fn removed_on_agent_run_deleted() {
    let mut state = MaterializedState::default();
    let ar_id = AgentRunId::new("ar-1");

    state.apply_event(&Event::AgentRunCreated {
        id: ar_id.clone(),
        agent_name: "fixer".to_string(),
        command_name: "fix".to_string(),
        namespace: "myproj".to_string(),
        cwd: PathBuf::from("/work"),
        runbook_hash: "abc".to_string(),
        vars: HashMap::new(),
        created_at_epoch_ms: 1_000,
    });

    state.apply_event(&Event::AgentRunStarted {
        id: ar_id.clone(),
        agent_id: oj_core::AgentId::new("agent-1"),
    });

    assert!(state.agents.contains_key("agent-1"));

    state.apply_event(&Event::AgentRunDeleted { id: ar_id });

    assert!(!state.agents.contains_key("agent-1"));
}

#[test]
fn idempotent_step_started() {
    let mut state = state_with_job_agent("pipe-1", "agent-1");

    // Apply again — should not panic or duplicate
    state.apply_event(&step_started_with_agent("pipe-1", "agent-1"));

    assert_eq!(state.agents.len(), 1);
    assert!(state.agents.contains_key("agent-1"));
}
