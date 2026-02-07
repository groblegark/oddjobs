// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use super::*;

#[test]
fn worker_started_with_queue_and_concurrency() {
    let mut state = MaterializedState::default();
    state.apply_event(&Event::WorkerStarted {
        worker_name: "fixer".to_string(),
        project_root: PathBuf::from("/test/project"),
        runbook_hash: "abc123".to_string(),
        queue_name: "bugs".to_string(),
        concurrency: 3,
        namespace: String::new(),
    });
    let worker = &state.workers["fixer"];
    assert_eq!(worker.status, "running");
    assert_eq!(worker.queue_name, "bugs");
    assert_eq!(worker.concurrency, 3);
    assert!(worker.active_job_ids.is_empty());
}

#[test]
fn worker_stopped_sets_status() {
    let mut state = MaterializedState::default();
    state.apply_event(&worker_start_event("fixer", ""));
    assert_eq!(state.workers["fixer"].status, "running");

    state.apply_event(&Event::WorkerStopped {
        worker_name: "fixer".to_string(),
        namespace: String::new(),
    });
    assert_eq!(state.workers["fixer"].status, "stopped");
}

#[test]
fn worker_record_backward_compat_missing_fields() {
    let json = r#"{
        "jobs": {},
        "sessions": {},
        "workspaces": {},
        "workers": {
            "old-worker": {
                "name": "old-worker",
                "project_root": "/tmp",
                "runbook_hash": "abc",
                "status": "running",
                "active_job_ids": []
            }
        },
        "runbooks": {}
    }"#;

    let state: MaterializedState = serde_json::from_str(json).unwrap();
    let worker = &state.workers["old-worker"];
    assert_eq!(worker.queue_name, "");
    assert_eq!(worker.concurrency, 0);
}

#[yare::parameterized(
    no_namespace   = { "", "fixer" },
    with_namespace = { "myproject", "myproject/fixer" },
)]
fn worker_started_preserves_active_job_ids(ns: &str, worker_key: &str) {
    let mut state = MaterializedState::default();

    state.apply_event(&worker_start_event("fixer", ns));
    state.apply_event(&Event::WorkerItemDispatched {
        worker_name: "fixer".to_string(),
        item_id: "item-1".to_string(),
        job_id: JobId::new("pipe-1"),
        namespace: ns.to_string(),
    });
    state.apply_event(&Event::WorkerItemDispatched {
        worker_name: "fixer".to_string(),
        item_id: "item-2".to_string(),
        job_id: JobId::new("pipe-2"),
        namespace: ns.to_string(),
    });

    assert_eq!(state.workers[worker_key].active_job_ids.len(), 2);

    // Simulate daemon restart: WorkerStarted replayed from WAL
    state.apply_event(&worker_start_event("fixer", ns));

    let worker = &state.workers[worker_key];
    assert_eq!(worker.active_job_ids.len(), 2);
    assert!(worker.active_job_ids.contains(&"pipe-1".to_string()));
    assert!(worker.active_job_ids.contains(&"pipe-2".to_string()));
}

#[test]
fn worker_deleted_lifecycle_and_ghost() {
    let mut state = MaterializedState::default();

    // Namespaced worker: start → stop → delete
    state.apply_event(&worker_start_event("fixer", "myproject"));
    assert_eq!(state.workers["myproject/fixer"].status, "running");
    state.apply_event(&Event::WorkerStopped {
        worker_name: "fixer".to_string(),
        namespace: "myproject".to_string(),
    });
    assert_eq!(state.workers["myproject/fixer"].status, "stopped");
    state.apply_event(&Event::WorkerDeleted {
        worker_name: "fixer".to_string(),
        namespace: "myproject".to_string(),
    });
    assert!(!state.workers.contains_key("myproject/fixer"));

    // Ghost worker (empty namespace): start → delete
    state.apply_event(&worker_start_event("ghost", ""));
    assert!(state.workers.contains_key("ghost"));
    state.apply_event(&Event::WorkerDeleted {
        worker_name: "ghost".to_string(),
        namespace: String::new(),
    });
    assert!(!state.workers.contains_key("ghost"));

    // Delete nonexistent worker is a no-op
    state.apply_event(&Event::WorkerDeleted {
        worker_name: "nonexistent".to_string(),
        namespace: String::new(),
    });
    assert!(state.workers.is_empty());
}
