// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use parking_lot::Mutex;
use tempfile::tempdir;

use oj_core::{StepOutcome, StepStatus, StepStatusKind};

use oj_engine::breadcrumb::{Breadcrumb, BreadcrumbAgent};

use super::{empty_state, handle_query, make_breadcrumb, make_job, Query, Response};

#[test]
fn list_jobs_includes_orphans() {
    let state = empty_state();
    let temp = tempdir().unwrap();
    let start = Instant::now();
    let orphans = Arc::new(Mutex::new(vec![make_breadcrumb(
        "orphan-1234",
        "fix/orphan",
        "oddjobs",
        "work",
    )]));

    let response = handle_query(Query::ListJobs, &state, &orphans, temp.path(), start);
    match response {
        Response::Jobs { jobs } => {
            assert_eq!(jobs.len(), 1);
            assert_eq!(jobs[0].id, "orphan-1234");
            assert_eq!(jobs[0].name, "fix/orphan");
            assert_eq!(jobs[0].step_status, StepStatusKind::Orphaned);
            assert_eq!(jobs[0].namespace, "oddjobs");
            assert!(jobs[0].updated_at_ms > 0);
        }
        other => panic!("unexpected response: {:?}", other),
    }
}

#[test]
fn get_job_falls_back_to_orphan() {
    let state = empty_state();
    let temp = tempdir().unwrap();
    let start = Instant::now();
    let orphans = Arc::new(Mutex::new(vec![make_breadcrumb(
        "orphan-5678",
        "fix/orphan",
        "oddjobs",
        "work",
    )]));

    let response = handle_query(
        Query::GetJob {
            id: "orphan-5678".to_string(),
        },
        &state,
        &orphans,
        temp.path(),
        start,
    );
    match response {
        Response::Job { job } => {
            let p = job.expect("should find orphan job");
            assert_eq!(p.id, "orphan-5678");
            assert_eq!(p.step_status, StepStatusKind::Orphaned);
            assert_eq!(p.session_id.as_deref(), Some("tmux-orphan-1"));
            assert!(p.error.is_some());
            assert_eq!(p.agents.len(), 1);
            assert_eq!(p.agents[0].status, "orphaned");
        }
        other => panic!("unexpected response: {:?}", other),
    }
}

#[test]
fn get_job_prefers_state_over_orphan() {
    let state = empty_state();
    let temp = tempdir().unwrap();
    let start = Instant::now();

    // Add job to both state and orphans with same ID
    {
        let mut s = state.lock();
        s.jobs.insert(
            "shared-id".to_string(),
            make_job(
                "shared-id",
                "fix/real",
                "oddjobs",
                "work",
                StepStatus::Running,
                StepOutcome::Running,
                None,
                1000,
            ),
        );
    }
    let orphans = Arc::new(Mutex::new(vec![make_breadcrumb(
        "shared-id",
        "fix/orphan",
        "oddjobs",
        "work",
    )]));

    let response = handle_query(
        Query::GetJob {
            id: "shared-id".to_string(),
        },
        &state,
        &orphans,
        temp.path(),
        start,
    );
    match response {
        Response::Job { job } => {
            let p = job.expect("should find job");
            // State version should be returned, not orphan
            assert_eq!(p.name, "fix/real");
            assert_ne!(p.step_status, StepStatusKind::Orphaned);
        }
        other => panic!("unexpected response: {:?}", other),
    }
}

#[test]
fn get_job_orphan_prefix_match() {
    let state = empty_state();
    let temp = tempdir().unwrap();
    let start = Instant::now();
    let orphans = Arc::new(Mutex::new(vec![make_breadcrumb(
        "orphan-abcdef123456",
        "fix/orphan",
        "oddjobs",
        "work",
    )]));

    // Use prefix to match
    let response = handle_query(
        Query::GetJob {
            id: "orphan-abcdef".to_string(),
        },
        &state,
        &orphans,
        temp.path(),
        start,
    );
    match response {
        Response::Job { job } => {
            let p = job.expect("should find orphan by prefix");
            assert_eq!(p.id, "orphan-abcdef123456");
            assert_eq!(p.step_status, StepStatusKind::Orphaned);
        }
        other => panic!("unexpected response: {:?}", other),
    }
}

#[test]
fn get_job_orphan_session_id_from_non_first_agent() {
    let state = empty_state();
    let temp = tempdir().unwrap();
    let start = Instant::now();

    // Breadcrumb with multiple agents; only the second has session_name set
    let orphans = Arc::new(Mutex::new(vec![Breadcrumb {
        job_id: "orphan-multi-agent".to_string(),
        project: "oddjobs".to_string(),
        kind: "command".to_string(),
        name: "fix/multi".to_string(),
        vars: HashMap::new(),
        current_step: "deploy".to_string(),
        step_status: "running".to_string(),
        agents: vec![
            BreadcrumbAgent {
                agent_id: "agent-plan".to_string(),
                session_name: None,
                log_path: std::path::PathBuf::from("/tmp/agent-plan.log"),
            },
            BreadcrumbAgent {
                agent_id: "agent-deploy".to_string(),
                session_name: Some("tmux-deploy".to_string()),
                log_path: std::path::PathBuf::from("/tmp/agent-deploy.log"),
            },
        ],
        workspace_id: None,
        workspace_root: None,
        updated_at: "2026-01-15T10:30:00Z".to_string(),
        runbook_hash: String::new(),
        cwd: None,
    }]));

    let response = handle_query(
        Query::GetJob {
            id: "orphan-multi-agent".to_string(),
        },
        &state,
        &orphans,
        temp.path(),
        start,
    );
    match response {
        Response::Job { job } => {
            let p = job.expect("should find orphan job");
            assert_eq!(
                p.session_id.as_deref(),
                Some("tmux-deploy"),
                "session_id should come from the agent that has session_name set, not just the first"
            );
        }
        other => panic!("unexpected response: {:?}", other),
    }
}

#[test]
fn get_job_logs_resolves_orphan_prefix() {
    let state = empty_state();
    let temp = tempdir().unwrap();
    let start = Instant::now();

    // Create the job log directory and file
    let log_dir = temp.path().join("job");
    std::fs::create_dir_all(&log_dir).unwrap();
    std::fs::write(
        log_dir.join("orphan-logs-full-id.log"),
        "line1\nline2\nline3\n",
    )
    .unwrap();

    let orphans = Arc::new(Mutex::new(vec![make_breadcrumb(
        "orphan-logs-full-id",
        "fix/orphan-logs",
        "oddjobs",
        "work",
    )]));

    // Use a prefix to look up logs
    let response = handle_query(
        Query::GetJobLogs {
            id: "orphan-logs".to_string(),
            lines: 0,
        },
        &state,
        &orphans,
        temp.path(),
        start,
    );
    match response {
        Response::JobLogs { log_path, content } => {
            assert!(
                log_path.ends_with("orphan-logs-full-id.log"),
                "log_path should use the full orphan ID, got: {:?}",
                log_path,
            );
            assert_eq!(content, "line1\nline2\nline3\n");
        }
        other => panic!("unexpected response: {:?}", other),
    }
}
