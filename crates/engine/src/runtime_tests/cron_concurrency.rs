// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Job-targeted cron concurrency tests

use super::*;

use super::cron::load_runbook;

const CRON_JOB_CONC_RUNBOOK: &str = r#"
[cron.deployer]
interval = "10m"
concurrency = 1
run = { job = "deploy" }

[job.deploy]
[[job.deploy.step]]
name = "run"
run = "echo deploying"
on_done = "done"

[[job.deploy.step]]
name = "done"
run = "echo finished"
"#;

const CRON_JOB_CONC2_RUNBOOK: &str = r#"
[cron.deployer]
interval = "10m"
concurrency = 2
run = { job = "deploy" }

[job.deploy]
[[job.deploy.step]]
name = "run"
run = "echo deploying"
on_done = "done"

[[job.deploy.step]]
name = "done"
run = "echo finished"
"#;

const CRON_JOB_NO_CONC_RUNBOOK: &str = r#"
[cron.deployer]
interval = "10m"
run = { job = "deploy" }

[job.deploy]
[[job.deploy.step]]
name = "run"
run = "echo deploying"
on_done = "done"

[[job.deploy.step]]
name = "done"
run = "echo finished"
"#;

// ---- Test 17: cron_job_concurrency_skip ----

#[tokio::test]
async fn cron_job_concurrency_skip() {
    let ctx = setup_with_runbook(CRON_JOB_CONC_RUNBOOK).await;
    let (runbook_json, runbook_hash) = hash_runbook(CRON_JOB_CONC_RUNBOOK);

    load_runbook(&ctx, &runbook_json, &runbook_hash).await;

    // Inject an active (non-terminal) job with cron_name = Some("deployer")
    ctx.runtime
        .executor
        .execute(oj_core::Effect::Emit {
            event: Event::JobCreated {
                id: JobId::new("existing-pipe-1"),
                kind: "deploy".to_string(),
                name: "deploy/existing".to_string(),
                runbook_hash: runbook_hash.clone(),
                cwd: ctx.project_root.clone(),
                vars: HashMap::new(),
                initial_step: "run".to_string(),
                created_at_epoch_ms: 1000,
                namespace: String::new(),
                cron_name: Some("deployer".to_string()),
            },
        })
        .await
        .unwrap();

    // Verify count_active_cron_jobs sees it
    assert_eq!(
        ctx.runtime.count_active_cron_jobs("deployer", ""),
        1,
        "should count 1 active cron job"
    );

    // Start cron with concurrency=1
    ctx.runtime
        .handle_event(Event::CronStarted {
            cron_name: "deployer".to_string(),
            project_root: ctx.project_root.clone(),
            runbook_hash: runbook_hash.clone(),
            interval: "10m".to_string(),
            run_target: "job:deploy".to_string(),
            namespace: String::new(),
        })
        .await
        .unwrap();

    // Fire the timer — should skip due to concurrency
    let events = ctx
        .runtime
        .handle_event(Event::TimerStart {
            id: oj_core::TimerId::cron("deployer", ""),
        })
        .await
        .unwrap();

    // No JobCreated should be emitted (spawn was skipped)
    let has_new_job = events
        .iter()
        .any(|e| matches!(e, Event::JobCreated { id, .. } if id.as_str() != "existing-pipe-1"));
    assert!(!has_new_job, "should NOT spawn job when at max concurrency");

    // No CronFired should be emitted (spawn was skipped)
    let has_cron_fired = events.iter().any(|e| matches!(e, Event::CronFired { .. }));
    assert!(
        !has_cron_fired,
        "CronFired should NOT be emitted when spawn is skipped"
    );

    // Timer should still be rescheduled
    let scheduler = ctx.runtime.executor.scheduler();
    let sched = scheduler.lock();
    assert!(
        sched.has_timers(),
        "timer should be rescheduled after concurrency skip"
    );
}

// ---- Test 18: cron_job_concurrency_respawns_after_complete ----

#[tokio::test]
async fn cron_job_concurrency_respawns_after_complete() {
    let ctx = setup_with_runbook(CRON_JOB_CONC_RUNBOOK).await;
    let (runbook_json, runbook_hash) = hash_runbook(CRON_JOB_CONC_RUNBOOK);

    load_runbook(&ctx, &runbook_json, &runbook_hash).await;

    // Inject a completed (terminal) job with cron_name = Some("deployer")
    ctx.runtime
        .executor
        .execute(oj_core::Effect::Emit {
            event: Event::JobCreated {
                id: JobId::new("completed-pipe-1"),
                kind: "deploy".to_string(),
                name: "deploy/completed".to_string(),
                runbook_hash: runbook_hash.clone(),
                cwd: ctx.project_root.clone(),
                vars: HashMap::new(),
                initial_step: "run".to_string(),
                created_at_epoch_ms: 1000,
                namespace: String::new(),
                cron_name: Some("deployer".to_string()),
            },
        })
        .await
        .unwrap();

    // Advance it to terminal state
    ctx.runtime
        .executor
        .execute(oj_core::Effect::Emit {
            event: Event::JobAdvanced {
                id: JobId::new("completed-pipe-1"),
                step: "done".to_string(),
            },
        })
        .await
        .unwrap();

    // Verify it doesn't count as active
    assert_eq!(
        ctx.runtime.count_active_cron_jobs("deployer", ""),
        0,
        "completed job should not count as active"
    );

    // Start cron with concurrency=1
    ctx.runtime
        .handle_event(Event::CronStarted {
            cron_name: "deployer".to_string(),
            project_root: ctx.project_root.clone(),
            runbook_hash: runbook_hash.clone(),
            interval: "10m".to_string(),
            run_target: "job:deploy".to_string(),
            namespace: String::new(),
        })
        .await
        .unwrap();

    // Fire the timer — should succeed since previous job is completed
    let events = ctx
        .runtime
        .handle_event(Event::TimerStart {
            id: oj_core::TimerId::cron("deployer", ""),
        })
        .await
        .unwrap();

    // JobCreated should be emitted (spawn succeeded)
    let has_job = events.iter().any(|e| matches!(e, Event::JobCreated { .. }));
    assert!(has_job, "should spawn job when previous run is completed");

    // CronFired should be emitted
    let has_cron_fired = events
        .iter()
        .any(|e| matches!(e, Event::CronFired { cron_name, .. } if cron_name == "deployer"));
    assert!(
        has_cron_fired,
        "CronFired should be emitted after successful spawn"
    );
}

// ---- Test 19: cron_job_concurrency_default_singleton ----

#[tokio::test]
async fn cron_job_concurrency_default_singleton() {
    let ctx = setup_with_runbook(CRON_JOB_NO_CONC_RUNBOOK).await;
    let (runbook_json, runbook_hash) = hash_runbook(CRON_JOB_NO_CONC_RUNBOOK);

    load_runbook(&ctx, &runbook_json, &runbook_hash).await;

    // Inject an active job with matching cron_name
    ctx.runtime
        .executor
        .execute(oj_core::Effect::Emit {
            event: Event::JobCreated {
                id: JobId::new("active-pipe-1"),
                kind: "deploy".to_string(),
                name: "deploy/active".to_string(),
                runbook_hash: runbook_hash.clone(),
                cwd: ctx.project_root.clone(),
                vars: HashMap::new(),
                initial_step: "run".to_string(),
                created_at_epoch_ms: 1000,
                namespace: String::new(),
                cron_name: Some("deployer".to_string()),
            },
        })
        .await
        .unwrap();

    // Start cron (no concurrency field = default 1)
    ctx.runtime
        .handle_event(Event::CronStarted {
            cron_name: "deployer".to_string(),
            project_root: ctx.project_root.clone(),
            runbook_hash: runbook_hash.clone(),
            interval: "10m".to_string(),
            run_target: "job:deploy".to_string(),
            namespace: String::new(),
        })
        .await
        .unwrap();

    // Fire the timer
    let events = ctx
        .runtime
        .handle_event(Event::TimerStart {
            id: oj_core::TimerId::cron("deployer", ""),
        })
        .await
        .unwrap();

    // Spawn should be skipped (default concurrency=1 makes it singleton)
    let has_new_job = events
        .iter()
        .any(|e| matches!(e, Event::JobCreated { id, .. } if id.as_str() != "active-pipe-1"));
    assert!(
        !has_new_job,
        "default concurrency=1 should make cron singleton"
    );
}

// ---- Test 20: cron_job_concurrency_allows_multiple ----

#[tokio::test]
async fn cron_job_concurrency_allows_multiple() {
    let ctx = setup_with_runbook(CRON_JOB_CONC2_RUNBOOK).await;
    let (runbook_json, runbook_hash) = hash_runbook(CRON_JOB_CONC2_RUNBOOK);

    load_runbook(&ctx, &runbook_json, &runbook_hash).await;

    // Inject one active job with matching cron_name
    ctx.runtime
        .executor
        .execute(oj_core::Effect::Emit {
            event: Event::JobCreated {
                id: JobId::new("active-pipe-1"),
                kind: "deploy".to_string(),
                name: "deploy/active".to_string(),
                runbook_hash: runbook_hash.clone(),
                cwd: ctx.project_root.clone(),
                vars: HashMap::new(),
                initial_step: "run".to_string(),
                created_at_epoch_ms: 1000,
                namespace: String::new(),
                cron_name: Some("deployer".to_string()),
            },
        })
        .await
        .unwrap();

    // Start cron with concurrency=2
    ctx.runtime
        .handle_event(Event::CronStarted {
            cron_name: "deployer".to_string(),
            project_root: ctx.project_root.clone(),
            runbook_hash: runbook_hash.clone(),
            interval: "10m".to_string(),
            run_target: "job:deploy".to_string(),
            namespace: String::new(),
        })
        .await
        .unwrap();

    // Fire the timer
    let events = ctx
        .runtime
        .handle_event(Event::TimerStart {
            id: oj_core::TimerId::cron("deployer", ""),
        })
        .await
        .unwrap();

    // JobCreated SHOULD be emitted (1 < 2, room for another)
    let has_new_job = events
        .iter()
        .any(|e| matches!(e, Event::JobCreated { id, .. } if id.as_str() != "active-pipe-1"));
    assert!(
        has_new_job,
        "concurrency=2 should allow second job when only 1 active"
    );

    // CronFired should be emitted
    let has_cron_fired = events.iter().any(|e| matches!(e, Event::CronFired { .. }));
    assert!(
        has_cron_fired,
        "CronFired should be emitted for successful spawn"
    );
}
