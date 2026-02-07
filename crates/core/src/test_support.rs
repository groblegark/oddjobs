// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Shared test helpers for use across crates.
//!
//! Gated behind `#[cfg(any(test, feature = "test-support"))]`.

use crate::{AgentRunId, Event, JobId, OwnerId, SessionId};
use std::collections::HashMap;
use std::path::PathBuf;

// ── Event factory functions ─────────────────────────────────────────────────

pub fn job_create_event(id: &str, kind: &str, name: &str, initial_step: &str) -> Event {
    Event::JobCreated {
        id: JobId::new(id),
        kind: kind.to_string(),
        name: name.to_string(),
        runbook_hash: "testhash".to_string(),
        cwd: PathBuf::from("/test/project"),
        vars: HashMap::new(),
        initial_step: initial_step.to_string(),
        created_at_epoch_ms: 1_000_000,
        namespace: String::new(),
        cron_name: None,
    }
}

pub fn job_delete_event(id: &str) -> Event {
    Event::JobDeleted { id: JobId::new(id) }
}

pub fn job_transition_event(id: &str, step: &str) -> Event {
    Event::JobAdvanced {
        id: JobId::new(id),
        step: step.to_string(),
    }
}

pub fn step_started_event(job_id: &str) -> Event {
    Event::StepStarted {
        job_id: JobId::new(job_id),
        step: "init".to_string(),
        agent_id: None,
        agent_name: None,
    }
}

pub fn step_failed_event(job_id: &str, step: &str, error: &str) -> Event {
    Event::StepFailed {
        job_id: JobId::new(job_id),
        step: step.to_string(),
        error: error.to_string(),
    }
}

pub fn session_create_event(id: &str, job_id: &str) -> Event {
    Event::SessionCreated {
        id: SessionId::new(id),
        owner: OwnerId::Job(JobId::new(job_id)),
    }
}

pub fn session_delete_event(id: &str) -> Event {
    Event::SessionDeleted {
        id: SessionId::new(id),
    }
}

pub fn worker_start_event(name: &str, ns: &str) -> Event {
    Event::WorkerStarted {
        worker_name: name.to_string(),
        project_root: PathBuf::from("/test/project"),
        runbook_hash: "abc123".to_string(),
        queue_name: "queue".to_string(),
        concurrency: 1,
        namespace: ns.to_string(),
    }
}

pub fn queue_pushed_event(queue_name: &str, item_id: &str) -> Event {
    Event::QueuePushed {
        queue_name: queue_name.to_string(),
        item_id: item_id.to_string(),
        data: [
            ("title".to_string(), "Fix bug".to_string()),
            ("id".to_string(), "123".to_string()),
        ]
        .into_iter()
        .collect(),
        pushed_at_epoch_ms: 1_000_000,
        namespace: String::new(),
    }
}

pub fn queue_taken_event(queue_name: &str, item_id: &str, worker_name: &str) -> Event {
    Event::QueueTaken {
        queue_name: queue_name.to_string(),
        item_id: item_id.to_string(),
        worker_name: worker_name.to_string(),
        namespace: String::new(),
    }
}

pub fn queue_failed_event(queue_name: &str, item_id: &str, error: &str) -> Event {
    Event::QueueFailed {
        queue_name: queue_name.to_string(),
        item_id: item_id.to_string(),
        error: error.to_string(),
        namespace: String::new(),
    }
}

pub fn agent_run_created_event(id: &str, agent_name: &str, command_name: &str) -> Event {
    Event::AgentRunCreated {
        id: AgentRunId::new(id),
        agent_name: agent_name.to_string(),
        command_name: command_name.to_string(),
        namespace: String::new(),
        cwd: PathBuf::from("/test/project"),
        runbook_hash: "testhash".to_string(),
        vars: HashMap::new(),
        created_at_epoch_ms: 1_000_000,
    }
}
