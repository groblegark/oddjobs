// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Unit tests for job lifecycle handling (resume)

use crate::{RuntimeConfig, RuntimeDeps};
use oj_adapters::{FakeAgentAdapter, FakeNotifyAdapter, FakeSessionAdapter};
use oj_core::{AgentId, Event, FakeClock, JobId, StepOutcome};
use oj_storage::MaterializedState;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;
use tempfile::tempdir;
use tokio::sync::mpsc;

type TestRuntime =
    crate::runtime::Runtime<FakeSessionAdapter, FakeAgentAdapter, FakeNotifyAdapter, FakeClock>;

struct TestContext {
    runtime: TestRuntime,
    #[allow(dead_code)]
    project_root: PathBuf,
}

/// Runbook with an agent step for testing resume
const AGENT_RUNBOOK: &str = r#"
[job.build]
input = ["prompt"]

[[job.build.step]]
name = "plan"
run = { agent = "planner" }
on_done = "done"
on_fail = "failed"

[[job.build.step]]
name = "done"
run = "echo done"

[[job.build.step]]
name = "failed"
run = "echo failed"

[agent.planner]
run = "claude"
prompt = "${var.prompt}"
"#;

async fn setup_with_runbook(runbook_content: &str) -> TestContext {
    let dir = tempdir().unwrap();
    let dir_path = dir.keep();

    let runbook_dir = dir_path.join(".oj/runbooks");
    std::fs::create_dir_all(&runbook_dir).unwrap();
    std::fs::write(runbook_dir.join("test.toml"), runbook_content).unwrap();

    let sessions = FakeSessionAdapter::new();
    let agents = FakeAgentAdapter::new();
    let notifier = FakeNotifyAdapter::new();
    let clock = FakeClock::new();
    let (event_tx, _event_rx) = mpsc::channel(100);
    let runtime = TestRuntime::new(
        RuntimeDeps {
            sessions,
            agents,
            notifier,
            state: Arc::new(Mutex::new(MaterializedState::default())),
        },
        clock,
        RuntimeConfig {
            state_dir: dir_path.clone(),
            log_dir: dir_path.join("logs"),
        },
        event_tx,
    );

    TestContext {
        runtime,
        project_root: dir_path,
    }
}

/// Parse a runbook, load it into cache + state, and return its hash.
fn load_runbook_hash(ctx: &TestContext, content: &str) -> String {
    let runbook = oj_runbook::parse_runbook(content).unwrap();
    let runbook_json = serde_json::to_value(&runbook).unwrap();
    let hash = {
        use sha2::{Digest, Sha256};
        let canonical = serde_json::to_string(&runbook_json).unwrap();
        let digest = Sha256::digest(canonical.as_bytes());
        format!("{:x}", digest)
    };
    {
        let mut cache = ctx.runtime.runbook_cache.lock();
        cache.insert(hash.clone(), runbook);
    }
    ctx.runtime.lock_state_mut(|state| {
        state.apply_event(&Event::RunbookLoaded {
            hash: hash.clone(),
            version: 1,
            runbook: runbook_json,
        });
    });
    hash
}

/// Create a job in "failed" state with a step history showing a failed "plan" step.
fn create_failed_job(ctx: &TestContext, job_id: &str, runbook_hash: &str) {
    let events = vec![
        Event::JobCreated {
            id: JobId::new(job_id),
            kind: "build".to_string(),
            name: "test-build".to_string(),
            runbook_hash: runbook_hash.to_string(),
            cwd: ctx.project_root.clone(),
            vars: HashMap::from([("prompt".to_string(), "Build feature".to_string())]),
            initial_step: "plan".to_string(),
            created_at_epoch_ms: 1_000_000,
            namespace: String::new(),
            cron_name: None,
        },
        // Agent started on "plan" step
        Event::StepStarted {
            job_id: JobId::new(job_id),
            step: "plan".to_string(),
            agent_id: Some(AgentId::new("agent-1")),
            agent_name: Some("planner".to_string()),
        },
        // Step failed
        Event::StepFailed {
            job_id: JobId::new(job_id),
            step: "plan".to_string(),
            error: "something went wrong".to_string(),
        },
        // Job transitioned to "failed" terminal state
        Event::JobAdvanced {
            id: JobId::new(job_id),
            step: "failed".to_string(),
        },
    ];
    ctx.runtime.lock_state_mut(|state| {
        for event in &events {
            state.apply_event(event);
        }
    });
}

/// Create a job in running state on agent step "plan".
fn create_running_job(ctx: &TestContext, job_id: &str, runbook_hash: &str) {
    let events = vec![
        Event::JobCreated {
            id: JobId::new(job_id),
            kind: "build".to_string(),
            name: "test-build".to_string(),
            runbook_hash: runbook_hash.to_string(),
            cwd: ctx.project_root.clone(),
            vars: HashMap::from([("prompt".to_string(), "Build feature".to_string())]),
            initial_step: "plan".to_string(),
            created_at_epoch_ms: 1_000_000,
            namespace: String::new(),
            cron_name: None,
        },
        Event::StepStarted {
            job_id: JobId::new(job_id),
            step: "plan".to_string(),
            agent_id: Some(AgentId::new("agent-1")),
            agent_name: Some("planner".to_string()),
        },
    ];
    ctx.runtime.lock_state_mut(|state| {
        for event in &events {
            state.apply_event(event);
        }
    });
}

// ============================================================================
// handle_job_resume: resume from failure with None message
// ============================================================================

#[tokio::test]
async fn resume_failed_job_with_none_message_uses_default() {
    let ctx = setup_with_runbook(AGENT_RUNBOOK).await;
    let hash = load_runbook_hash(&ctx, AGENT_RUNBOOK);
    create_failed_job(&ctx, "job-1", &hash);

    // Verify job is in "failed" state
    let job = ctx.runtime.lock_state(|s| s.jobs.get("job-1").cloned());
    assert_eq!(job.as_ref().unwrap().step, "failed");

    // Resume with no message — should succeed with default "Retrying"
    let job_id = JobId::new("job-1");
    let result = ctx
        .runtime
        .handle_job_resume(&job_id, None, &HashMap::new(), false)
        .await;

    // Should succeed (not error about missing message)
    assert!(result.is_ok(), "expected Ok, got: {:?}", result.err());
}

#[tokio::test]
async fn resume_failed_job_returns_job_advanced_event() {
    let ctx = setup_with_runbook(AGENT_RUNBOOK).await;
    let hash = load_runbook_hash(&ctx, AGENT_RUNBOOK);
    create_failed_job(&ctx, "job-1", &hash);

    let job_id = JobId::new("job-1");
    let result = ctx
        .runtime
        .handle_job_resume(&job_id, Some("Try again"), &HashMap::new(), false)
        .await;

    let events = result.unwrap();
    // Should contain a JobAdvanced event (for WAL persistence)
    let has_job_advanced = events.iter().any(|e| {
        matches!(e, Event::JobAdvanced { id, step } if id.as_str() == "job-1" && step == "plan")
    });
    assert!(
        has_job_advanced,
        "expected JobAdvanced event in result for WAL persistence, got: {:?}",
        events
    );
}

// ============================================================================
// handle_job_resume: running job with None message should error
// ============================================================================

#[tokio::test]
async fn resume_running_agent_job_without_message_errors() {
    let ctx = setup_with_runbook(AGENT_RUNBOOK).await;
    let hash = load_runbook_hash(&ctx, AGENT_RUNBOOK);
    create_running_job(&ctx, "job-1", &hash);

    // Verify job is on "plan" step (running)
    let job = ctx.runtime.lock_state(|s| s.jobs.get("job-1").cloned());
    assert_eq!(job.as_ref().unwrap().step, "plan");

    // Resume with no message — should error
    let job_id = JobId::new("job-1");
    let result = ctx
        .runtime
        .handle_job_resume(&job_id, None, &HashMap::new(), false)
        .await;

    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("--message"),
        "expected error about missing --message, got: {}",
        err_msg
    );
}

#[tokio::test]
async fn resume_running_agent_job_without_message_does_not_mutate_state() {
    let ctx = setup_with_runbook(AGENT_RUNBOOK).await;
    let hash = load_runbook_hash(&ctx, AGENT_RUNBOOK);
    create_running_job(&ctx, "job-1", &hash);

    // Snapshot step history before resume attempt
    let history_before = ctx.runtime.lock_state(|s| {
        s.jobs
            .get("job-1")
            .map(|j| j.step_history.clone())
            .unwrap_or_default()
    });

    // Attempt resume with no message (should fail)
    let job_id = JobId::new("job-1");
    let _ = ctx
        .runtime
        .handle_job_resume(&job_id, None, &HashMap::new(), false)
        .await;

    // Verify state was NOT mutated (no JobAdvanced emitted)
    let job = ctx.runtime.lock_state(|s| s.jobs.get("job-1").cloned());
    let job = job.unwrap();
    assert_eq!(job.step, "plan", "step should not have changed");
    assert_eq!(
        job.step_history.len(),
        history_before.len(),
        "step history should not have changed"
    );
}

// ============================================================================
// handle_job_resume: failed job step history has expected outcome
// ============================================================================

#[tokio::test]
async fn failed_job_has_failed_step_in_history() {
    let ctx = setup_with_runbook(AGENT_RUNBOOK).await;
    let hash = load_runbook_hash(&ctx, AGENT_RUNBOOK);
    create_failed_job(&ctx, "job-1", &hash);

    let job = ctx
        .runtime
        .lock_state(|s| s.jobs.get("job-1").cloned())
        .unwrap();
    assert_eq!(job.step, "failed");

    // Verify step history contains a failed "plan" step
    let failed_step = job
        .step_history
        .iter()
        .find(|r| r.name == "plan" && matches!(r.outcome, StepOutcome::Failed(_)));
    assert!(
        failed_step.is_some(),
        "expected a failed 'plan' step in history"
    );
}
