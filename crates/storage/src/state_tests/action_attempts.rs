// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use super::*;

fn set_action_attempts(state: &mut MaterializedState, job_id: &str, action: &str, n: usize) {
    let job = state.jobs.get_mut(job_id).unwrap();
    for _ in 0..n {
        job.increment_action_attempt(action, 0);
    }
}

#[test]
fn on_done_transition_resets_action_attempts() {
    let mut state = MaterializedState::default();
    state.apply_event(&job_create_event("pipe-1", "build", "test", "init"));
    set_action_attempts(&mut state, "pipe-1", "exit", 2);
    assert_eq!(state.jobs["pipe-1"].get_action_attempt("exit", 0), 2);

    state.apply_event(&Event::StepCompleted {
        job_id: JobId::new("pipe-1"),
        step: "init".to_string(),
    });
    state.apply_event(&job_transition_event("pipe-1", "plan"));

    assert_eq!(state.jobs["pipe-1"].get_action_attempt("exit", 0), 0);
}

#[test]
fn on_fail_transition_preserves_action_attempts() {
    let mut state = MaterializedState::default();
    state.apply_event(&job_create_event("pipe-1", "build", "test", "work"));
    set_action_attempts(&mut state, "pipe-1", "exit", 2);
    assert_eq!(state.jobs["pipe-1"].get_action_attempt("exit", 0), 2);

    state.apply_event(&step_failed_event("pipe-1", "work", "agent exited"));
    state.apply_event(&job_transition_event("pipe-1", "recover"));

    assert_eq!(
        state.jobs["pipe-1"].get_action_attempt("exit", 0),
        2,
        "action_attempts must be preserved on on_fail transition"
    );
}

#[test]
fn on_fail_same_step_cycle_preserves_action_attempts() {
    let mut state = MaterializedState::default();
    state.apply_event(&job_create_event("pipe-1", "build", "test", "work"));
    set_action_attempts(&mut state, "pipe-1", "exit", 1);
    assert_eq!(state.jobs["pipe-1"].get_action_attempt("exit", 0), 1);

    state.apply_event(&step_failed_event("pipe-1", "work", "agent exited"));
    state.apply_event(&job_transition_event("pipe-1", "work"));

    assert_eq!(
        state.jobs["pipe-1"].get_action_attempt("exit", 0),
        1,
        "action_attempts must be preserved on same-step on_fail cycle"
    );

    assert_eq!(state.jobs["pipe-1"].step, "work");
    assert_eq!(
        state.jobs["pipe-1"].step_status,
        oj_core::StepStatus::Pending
    );
}

#[test]
fn on_fail_same_step_cycle_pushes_new_step_record() {
    let mut state = MaterializedState::default();
    state.apply_event(&job_create_event("pipe-1", "build", "test", "work"));

    assert_eq!(state.jobs["pipe-1"].step_history.len(), 1);

    state.apply_event(&step_failed_event("pipe-1", "work", "agent exited"));
    state.apply_event(&job_transition_event("pipe-1", "work"));

    assert_eq!(
        state.jobs["pipe-1"].step_history.len(),
        2,
        "same-step on_fail should push a new step record"
    );
    assert_eq!(
        state.jobs["pipe-1"].step_history[0].outcome,
        StepOutcome::Failed("agent exited".to_string())
    );
    assert!(state.jobs["pipe-1"].step_history[0]
        .finished_at_ms
        .is_some());
    assert_eq!(
        state.jobs["pipe-1"].step_history[1].outcome,
        StepOutcome::Running
    );
    assert!(state.jobs["pipe-1"].step_history[1]
        .finished_at_ms
        .is_none());
}

#[test]
fn on_fail_multi_step_cycle_preserves_attempts_across_chain() {
    let mut state = MaterializedState::default();
    state.apply_event(&job_create_event("pipe-1", "build", "test", "step-a"));

    set_action_attempts(&mut state, "pipe-1", "exit", 1);

    // step-a fails → on_fail → step-b
    state.apply_event(&step_failed_event("pipe-1", "step-a", "failed"));
    state.apply_event(&job_transition_event("pipe-1", "step-b"));
    assert_eq!(state.jobs["pipe-1"].get_action_attempt("exit", 0), 1);

    set_action_attempts(&mut state, "pipe-1", "exit", 1);

    // step-b fails → on_fail → step-a (cycle)
    state.apply_event(&step_failed_event("pipe-1", "step-b", "failed"));
    state.apply_event(&job_transition_event("pipe-1", "step-a"));

    assert_eq!(
        state.jobs["pipe-1"].get_action_attempt("exit", 0),
        2,
        "action_attempts must accumulate across multi-step on_fail cycles"
    );
}
