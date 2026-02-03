// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Cron-related runtime tests

use super::*;
use crate::runtime::handlers::cron::CronStatus;

const CRON_RUNBOOK: &str = r#"
[cron.janitor]
interval = "30m"
run = { pipeline = "cleanup" }

[pipeline.cleanup]

[[pipeline.cleanup.step]]
name = "prune"
run = "echo pruning"
on_done = "done"

[[pipeline.cleanup.step]]
name = "done"
run = "echo finished"
"#;

/// Helper: parse the CRON_RUNBOOK and return (runbook_json, runbook_hash).
fn hash_cron_runbook() -> (serde_json::Value, String) {
    let runbook = oj_runbook::parse_runbook(CRON_RUNBOOK).unwrap();
    let runbook_json = serde_json::to_value(&runbook).unwrap();
    let runbook_hash = {
        use sha2::{Digest, Sha256};
        let canonical = serde_json::to_string(&runbook_json).unwrap();
        let digest = Sha256::digest(canonical.as_bytes());
        format!("{:x}", digest)
    };
    (runbook_json, runbook_hash)
}

/// Helper: emit RunbookLoaded to populate the engine's in-process cache.
async fn load_runbook(ctx: &TestContext, runbook_json: &serde_json::Value, runbook_hash: &str) {
    ctx.runtime
        .handle_event(Event::RunbookLoaded {
            hash: runbook_hash.to_string(),
            version: 1,
            runbook: runbook_json.clone(),
        })
        .await
        .unwrap();
}

// ---- Test 1: cron_once_creates_pipeline ----

#[tokio::test]
async fn cron_once_creates_pipeline() {
    let ctx = setup_with_runbook(CRON_RUNBOOK).await;
    let (runbook_json, runbook_hash) = hash_cron_runbook();

    // Populate runbook cache
    load_runbook(&ctx, &runbook_json, &runbook_hash).await;

    // Emit CronOnce event
    let pipeline_id = PipelineId::new("cron-pipe-1");
    let events = ctx
        .runtime
        .handle_event(Event::CronOnce {
            cron_name: "janitor".to_string(),
            pipeline_id: pipeline_id.clone(),
            pipeline_name: "cleanup/cron-pip".to_string(),
            pipeline_kind: "cleanup".to_string(),
            project_root: ctx.project_root.clone(),
            runbook_hash: runbook_hash.clone(),
            namespace: String::new(),
        })
        .await
        .unwrap();

    // Pipeline should be created
    let pipeline = ctx
        .runtime
        .get_pipeline("cron-pipe-1")
        .expect("pipeline should exist");
    assert_eq!(pipeline.kind, "cleanup");
    assert_eq!(pipeline.step, "prune");

    // CronFired tracking event should have been emitted
    let has_cron_fired = events
        .iter()
        .any(|e| matches!(e, Event::CronFired { cron_name, .. } if cron_name == "janitor"));
    assert!(has_cron_fired, "CronFired event should be emitted");
}

// ---- Test 2: cron_start_sets_timer ----

#[tokio::test]
async fn cron_start_sets_timer() {
    let ctx = setup_with_runbook(CRON_RUNBOOK).await;
    let (runbook_json, runbook_hash) = hash_cron_runbook();

    load_runbook(&ctx, &runbook_json, &runbook_hash).await;

    // Emit CronStarted
    ctx.runtime
        .handle_event(Event::CronStarted {
            cron_name: "janitor".to_string(),
            project_root: ctx.project_root.clone(),
            runbook_hash: runbook_hash.clone(),
            interval: "30m".to_string(),
            pipeline_name: "cleanup".to_string(),
            namespace: String::new(),
        })
        .await
        .unwrap();

    // Cron state should be Running
    {
        let crons = ctx.runtime.cron_states.lock();
        let state = crons.get("janitor").expect("cron state should exist");
        assert_eq!(state.status, CronStatus::Running);
        assert_eq!(state.pipeline_name, "cleanup");
        assert_eq!(state.interval, "30m");
    }

    // Timer should have been set
    let scheduler = ctx.runtime.executor.scheduler();
    let mut sched = scheduler.lock();
    assert!(sched.has_timers(), "timer should be set after CronStarted");

    // Advance clock past 30m and check that cron timer fires
    ctx.clock
        .advance(std::time::Duration::from_secs(30 * 60 + 1));
    let fired = sched.fired_timers(ctx.clock.now());
    let timer_ids: Vec<&str> = fired
        .iter()
        .filter_map(|e| match e {
            Event::TimerStart { id } => Some(id.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        timer_ids.iter().any(|id| id.starts_with("cron:")),
        "cron timer should fire after interval: {:?}",
        timer_ids
    );
}

// ---- Test 3: cron_stop_cancels_timer ----

#[tokio::test]
async fn cron_stop_cancels_timer() {
    let ctx = setup_with_runbook(CRON_RUNBOOK).await;
    let (runbook_json, runbook_hash) = hash_cron_runbook();

    load_runbook(&ctx, &runbook_json, &runbook_hash).await;

    // Start cron
    ctx.runtime
        .handle_event(Event::CronStarted {
            cron_name: "janitor".to_string(),
            project_root: ctx.project_root.clone(),
            runbook_hash: runbook_hash.clone(),
            interval: "30m".to_string(),
            pipeline_name: "cleanup".to_string(),
            namespace: String::new(),
        })
        .await
        .unwrap();

    // Stop cron
    ctx.runtime
        .handle_event(Event::CronStopped {
            cron_name: "janitor".to_string(),
            namespace: String::new(),
        })
        .await
        .unwrap();

    // Cron state should be Stopped
    {
        let crons = ctx.runtime.cron_states.lock();
        let state = crons.get("janitor").expect("cron state should exist");
        assert_eq!(state.status, CronStatus::Stopped);
    }

    // Timer should be cancelled (no timers remaining)
    let scheduler = ctx.runtime.executor.scheduler();
    let mut sched = scheduler.lock();
    ctx.clock
        .advance(std::time::Duration::from_secs(30 * 60 + 1));
    let fired = sched.fired_timers(ctx.clock.now());
    let cron_timers: Vec<&str> = fired
        .iter()
        .filter_map(|e| match e {
            Event::TimerStart { id } if id.as_str().starts_with("cron:") => Some(id.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        cron_timers.is_empty(),
        "no cron timers should fire after stop: {:?}",
        cron_timers
    );
}

// ---- Test 4: cron_timer_fired_creates_pipeline ----

#[tokio::test]
async fn cron_timer_fired_creates_pipeline() {
    let ctx = setup_with_runbook(CRON_RUNBOOK).await;
    let (runbook_json, runbook_hash) = hash_cron_runbook();

    load_runbook(&ctx, &runbook_json, &runbook_hash).await;

    // Start cron (registers state + sets timer)
    ctx.runtime
        .handle_event(Event::CronStarted {
            cron_name: "janitor".to_string(),
            project_root: ctx.project_root.clone(),
            runbook_hash: runbook_hash.clone(),
            interval: "30m".to_string(),
            pipeline_name: "cleanup".to_string(),
            namespace: String::new(),
        })
        .await
        .unwrap();

    // Simulate timer firing via TimerStart event
    let events = ctx
        .runtime
        .handle_event(Event::TimerStart {
            id: oj_core::TimerId::cron("janitor", ""),
        })
        .await
        .unwrap();

    // Pipeline should have been created
    let pipelines = ctx.runtime.pipelines();
    assert_eq!(pipelines.len(), 1, "one pipeline should be created");

    let pipeline = pipelines.values().next().unwrap();
    assert_eq!(pipeline.kind, "cleanup");
    assert_eq!(pipeline.step, "prune");

    // CronFired event should be in result
    let has_cron_fired = events
        .iter()
        .any(|e| matches!(e, Event::CronFired { cron_name, .. } if cron_name == "janitor"));
    assert!(has_cron_fired, "CronFired event should be emitted");
}

// ---- Test 5: cron_timer_fired_reloads_runbook ----

#[tokio::test]
async fn cron_timer_fired_reloads_runbook() {
    let ctx = setup_with_runbook(CRON_RUNBOOK).await;
    let (runbook_json, runbook_hash) = hash_cron_runbook();

    load_runbook(&ctx, &runbook_json, &runbook_hash).await;

    // Start cron
    ctx.runtime
        .handle_event(Event::CronStarted {
            cron_name: "janitor".to_string(),
            project_root: ctx.project_root.clone(),
            runbook_hash: runbook_hash.clone(),
            interval: "30m".to_string(),
            pipeline_name: "cleanup".to_string(),
            namespace: String::new(),
        })
        .await
        .unwrap();

    // Modify runbook on disk (add a comment to change the hash)
    let modified_runbook = r#"
[cron.janitor]
interval = "30m"
run = { pipeline = "cleanup" }

[pipeline.cleanup]

[[pipeline.cleanup.step]]
name = "prune"
run = "echo pruning v2"
on_done = "done"

[[pipeline.cleanup.step]]
name = "done"
run = "echo finished v2"
"#;
    let runbook_path = ctx.project_root.join(".oj/runbooks/test.toml");
    std::fs::write(&runbook_path, modified_runbook).unwrap();

    // Fire timer — should reload runbook from disk
    ctx.runtime
        .handle_event(Event::TimerStart {
            id: oj_core::TimerId::cron("janitor", ""),
        })
        .await
        .unwrap();

    // Verify runbook hash was updated in cron state
    let new_hash = {
        let crons = ctx.runtime.cron_states.lock();
        crons.get("janitor").unwrap().runbook_hash.clone()
    };
    assert_ne!(
        new_hash, runbook_hash,
        "runbook hash should change after modification"
    );
}

// ---- Test 6: cron_once_pipeline_steps_execute ----

#[tokio::test]
async fn cron_once_pipeline_steps_execute() {
    let ctx = setup_with_runbook(CRON_RUNBOOK).await;
    let (runbook_json, runbook_hash) = hash_cron_runbook();

    load_runbook(&ctx, &runbook_json, &runbook_hash).await;

    // Emit CronOnce
    let pipeline_id = PipelineId::new("cron-exec-1");
    ctx.runtime
        .handle_event(Event::CronOnce {
            cron_name: "janitor".to_string(),
            pipeline_id: pipeline_id.clone(),
            pipeline_name: "cleanup/cron-exe".to_string(),
            pipeline_kind: "cleanup".to_string(),
            project_root: ctx.project_root.clone(),
            runbook_hash: runbook_hash.clone(),
            namespace: String::new(),
        })
        .await
        .unwrap();

    // Pipeline should be at first step
    let pipeline = ctx.runtime.get_pipeline("cron-exec-1").unwrap();
    assert_eq!(pipeline.step, "prune");

    // Simulate shell completion of first step
    ctx.runtime
        .handle_event(Event::ShellExited {
            pipeline_id: pipeline_id.clone(),
            step: "prune".to_string(),
            exit_code: 0,
        })
        .await
        .unwrap();

    // Pipeline should advance to "done" step
    let pipeline = ctx.runtime.get_pipeline("cron-exec-1").unwrap();
    assert_eq!(pipeline.step, "done");

    // Complete the "done" step
    ctx.runtime
        .handle_event(Event::ShellExited {
            pipeline_id: pipeline_id.clone(),
            step: "done".to_string(),
            exit_code: 0,
        })
        .await
        .unwrap();

    // Pipeline should be terminal
    let pipeline = ctx.runtime.get_pipeline("cron-exec-1").unwrap();
    assert!(
        pipeline.is_terminal(),
        "pipeline should be terminal after all steps complete"
    );
}
