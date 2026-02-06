// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Agent-targeted cron tests

use super::*;
use crate::runtime::handlers::cron::{CronRunTarget, CronStatus};
use oj_core::AgentRunStatus;

use super::cron::load_runbook;

const CRON_AGENT_RUNBOOK: &str = r#"
[cron.health_check]
interval = "30m"
run = { agent = "doctor" }

[agent.doctor]
run = "claude --print"
prompt = "Run diagnostics"
"#;

const CRON_AGENT_MAX_CONC_RUNBOOK: &str = r#"
[cron.health_check]
interval = "30m"
run = { agent = "doctor" }

[agent.doctor]
max_concurrency = 1
run = "claude --print"
prompt = "Run diagnostics"
"#;

// ---- Test 10: cron_once_agent ----

#[tokio::test]
async fn cron_once_agent() {
    let ctx = setup_with_runbook(CRON_AGENT_RUNBOOK).await;
    let (runbook_json, runbook_hash) = hash_runbook(CRON_AGENT_RUNBOOK);

    load_runbook(&ctx, &runbook_json, &runbook_hash).await;

    // Emit CronOnce targeting an agent
    let events = ctx
        .runtime
        .handle_event(Event::CronOnce {
            cron_name: "health_check".to_string(),
            job_id: JobId::default(),
            job_name: String::new(),
            job_kind: String::new(),
            agent_run_id: Some("ar-once-1".to_string()),
            agent_name: Some("doctor".to_string()),
            project_root: ctx.project_root.clone(),
            runbook_hash: runbook_hash.clone(),
            run_target: "agent:doctor".to_string(),
            namespace: String::new(),
        })
        .await
        .unwrap();

    // CronFired event should be emitted with agent_run_id
    let cron_fired = events
        .iter()
        .find(|e| matches!(e, Event::CronFired { cron_name, .. } if cron_name == "health_check"));
    assert!(cron_fired.is_some(), "CronFired event should be emitted");
    if let Some(Event::CronFired { agent_run_id, .. }) = cron_fired {
        assert!(agent_run_id.is_some(), "agent_run_id should be set");
    }

    // AgentRunCreated event should be emitted
    let has_agent_run_created = events.iter().any(|e| {
        matches!(e, Event::AgentRunCreated { agent_name, command_name, .. }
            if agent_name == "doctor" && command_name == "cron:health_check")
    });
    assert!(
        has_agent_run_created,
        "AgentRunCreated should be emitted for cron-once agent"
    );
}

// ---- Test 11: cron_start_agent_sets_timer ----

#[tokio::test]
async fn cron_start_agent_sets_timer() {
    let ctx = setup_with_runbook(CRON_AGENT_RUNBOOK).await;
    let (runbook_json, runbook_hash) = hash_runbook(CRON_AGENT_RUNBOOK);

    load_runbook(&ctx, &runbook_json, &runbook_hash).await;

    // Emit CronStarted with agent target
    ctx.runtime
        .handle_event(Event::CronStarted {
            cron_name: "health_check".to_string(),
            project_root: ctx.project_root.clone(),
            runbook_hash: runbook_hash.clone(),
            interval: "30m".to_string(),
            run_target: "agent:doctor".to_string(),
            namespace: String::new(),
        })
        .await
        .unwrap();

    // Cron state should target an agent
    {
        let crons = ctx.runtime.cron_states.lock();
        let state = crons.get("health_check").expect("cron state should exist");
        assert_eq!(state.status, CronStatus::Running);
        assert!(
            matches!(state.run_target, CronRunTarget::Agent(ref a) if a == "doctor"),
            "run_target should be Agent(doctor)"
        );
    }

    // Timer should be set
    let scheduler = ctx.runtime.executor.scheduler();
    let sched = scheduler.lock();
    assert!(sched.has_timers(), "timer should be set");
}

// ---- Test 12: cron_timer_fires_agent ----

#[tokio::test]
async fn cron_timer_fires_agent() {
    let ctx = setup_with_runbook(CRON_AGENT_RUNBOOK).await;
    let (runbook_json, runbook_hash) = hash_runbook(CRON_AGENT_RUNBOOK);

    load_runbook(&ctx, &runbook_json, &runbook_hash).await;

    // Start cron targeting agent
    ctx.runtime
        .handle_event(Event::CronStarted {
            cron_name: "health_check".to_string(),
            project_root: ctx.project_root.clone(),
            runbook_hash: runbook_hash.clone(),
            interval: "30m".to_string(),
            run_target: "agent:doctor".to_string(),
            namespace: String::new(),
        })
        .await
        .unwrap();

    // Fire the timer
    let events = ctx
        .runtime
        .handle_event(Event::TimerStart {
            id: oj_core::TimerId::cron("health_check", ""),
        })
        .await
        .unwrap();

    // AgentRunCreated should be emitted
    let has_agent_run = events.iter().any(|e| {
        matches!(e, Event::AgentRunCreated { agent_name, command_name, .. }
            if agent_name == "doctor" && command_name == "cron:health_check")
    });
    assert!(
        has_agent_run,
        "AgentRunCreated should be emitted on cron timer fire"
    );

    // CronFired should be emitted with agent_run_id
    let has_cron_fired = events.iter().any(|e| {
        matches!(e, Event::CronFired { cron_name, agent_run_id, .. }
            if cron_name == "health_check" && agent_run_id.is_some())
    });
    assert!(
        has_cron_fired,
        "CronFired should be emitted with agent_run_id"
    );

    // No jobs should be created
    let jobs = ctx.runtime.jobs();
    assert!(jobs.is_empty(), "no jobs should be created for agent cron");
}

// ---- Test 13: cron_agent_concurrency_skip ----

#[tokio::test]
async fn cron_agent_concurrency_skip() {
    let ctx = setup_with_runbook(CRON_AGENT_MAX_CONC_RUNBOOK).await;
    let (runbook_json, runbook_hash) = hash_runbook(CRON_AGENT_MAX_CONC_RUNBOOK);

    load_runbook(&ctx, &runbook_json, &runbook_hash).await;

    // Inject a running agent into state (simulating an existing running instance)
    // Use executor.execute(Effect::Emit) so state.apply_event() is called
    ctx.runtime
        .executor
        .execute(oj_core::Effect::Emit {
            event: Event::AgentRunCreated {
                id: oj_core::AgentRunId::new("existing-run-1"),
                agent_name: "doctor".to_string(),
                command_name: "cron:health_check".to_string(),
                namespace: String::new(),
                cwd: ctx.project_root.clone(),
                runbook_hash: runbook_hash.clone(),
                vars: HashMap::new(),
                created_at_epoch_ms: 1000,
            },
        })
        .await
        .unwrap();

    // Verify count_running_agents sees it
    assert_eq!(
        ctx.runtime.count_running_agents("doctor", ""),
        1,
        "should count 1 running agent"
    );

    // Start cron targeting agent with max_concurrency=1
    ctx.runtime
        .handle_event(Event::CronStarted {
            cron_name: "health_check".to_string(),
            project_root: ctx.project_root.clone(),
            runbook_hash: runbook_hash.clone(),
            interval: "30m".to_string(),
            run_target: "agent:doctor".to_string(),
            namespace: String::new(),
        })
        .await
        .unwrap();

    // Fire the timer — should skip due to max_concurrency
    let events = ctx
        .runtime
        .handle_event(Event::TimerStart {
            id: oj_core::TimerId::cron("health_check", ""),
        })
        .await
        .unwrap();

    // No AgentRunCreated should be emitted (spawn was skipped)
    let has_new_agent = events
        .iter()
        .any(|e| matches!(e, Event::AgentRunCreated { agent_name, .. } if agent_name == "doctor"));
    assert!(
        !has_new_agent,
        "should NOT spawn agent when at max concurrency"
    );

    // No CronFired should be emitted (spawn was skipped)
    let has_cron_fired = events.iter().any(|e| matches!(e, Event::CronFired { .. }));
    assert!(
        !has_cron_fired,
        "CronFired should NOT be emitted when spawn is skipped"
    );

    // Timer should still be rescheduled (so it tries again next interval)
    let scheduler = ctx.runtime.executor.scheduler();
    let sched = scheduler.lock();
    assert!(
        sched.has_timers(),
        "timer should be rescheduled after concurrency skip"
    );
}

// ---- Test 14: cron_agent_concurrency_respawns_after_complete ----

#[tokio::test]
async fn cron_agent_concurrency_respawns_after_complete() {
    let ctx = setup_with_runbook(CRON_AGENT_MAX_CONC_RUNBOOK).await;
    let (runbook_json, runbook_hash) = hash_runbook(CRON_AGENT_MAX_CONC_RUNBOOK);

    load_runbook(&ctx, &runbook_json, &runbook_hash).await;

    // Inject a completed agent run (should NOT count against concurrency)
    ctx.runtime
        .executor
        .execute(oj_core::Effect::Emit {
            event: Event::AgentRunCreated {
                id: oj_core::AgentRunId::new("completed-run-1"),
                agent_name: "doctor".to_string(),
                command_name: "cron:health_check".to_string(),
                namespace: String::new(),
                cwd: ctx.project_root.clone(),
                runbook_hash: runbook_hash.clone(),
                vars: HashMap::new(),
                created_at_epoch_ms: 1000,
            },
        })
        .await
        .unwrap();

    // Mark it as completed
    ctx.runtime
        .executor
        .execute(oj_core::Effect::Emit {
            event: Event::AgentRunStatusChanged {
                id: oj_core::AgentRunId::new("completed-run-1"),
                status: AgentRunStatus::Completed,
                reason: None,
            },
        })
        .await
        .unwrap();

    // Should not count as running
    assert_eq!(
        ctx.runtime.count_running_agents("doctor", ""),
        0,
        "completed agent should not count as running"
    );

    // Start cron targeting agent with max_concurrency=1
    ctx.runtime
        .handle_event(Event::CronStarted {
            cron_name: "health_check".to_string(),
            project_root: ctx.project_root.clone(),
            runbook_hash: runbook_hash.clone(),
            interval: "30m".to_string(),
            run_target: "agent:doctor".to_string(),
            namespace: String::new(),
        })
        .await
        .unwrap();

    // Fire the timer — should succeed since previous run is completed
    let events = ctx
        .runtime
        .handle_event(Event::TimerStart {
            id: oj_core::TimerId::cron("health_check", ""),
        })
        .await
        .unwrap();

    // AgentRunCreated should be emitted (spawn succeeded)
    let has_agent_run = events
        .iter()
        .any(|e| matches!(e, Event::AgentRunCreated { agent_name, .. } if agent_name == "doctor"));
    assert!(
        has_agent_run,
        "should spawn agent when previous run is completed"
    );

    // CronFired should be emitted
    let has_cron_fired = events
        .iter()
        .any(|e| matches!(e, Event::CronFired { cron_name, .. } if cron_name == "health_check"));
    assert!(
        has_cron_fired,
        "CronFired should be emitted after successful spawn"
    );
}

// ---- Test 15: count_running_agents_standalone ----

#[tokio::test]
async fn count_running_agents_standalone() {
    let ctx = setup_with_runbook(CRON_AGENT_RUNBOOK).await;
    let (runbook_json, runbook_hash) = hash_runbook(CRON_AGENT_RUNBOOK);

    load_runbook(&ctx, &runbook_json, &runbook_hash).await;

    // Initially no running agents
    assert_eq!(ctx.runtime.count_running_agents("doctor", ""), 0);

    // Add a starting agent
    ctx.runtime
        .executor
        .execute(oj_core::Effect::Emit {
            event: Event::AgentRunCreated {
                id: oj_core::AgentRunId::new("run-1"),
                agent_name: "doctor".to_string(),
                command_name: "test".to_string(),
                namespace: String::new(),
                cwd: ctx.project_root.clone(),
                runbook_hash: runbook_hash.clone(),
                vars: HashMap::new(),
                created_at_epoch_ms: 1000,
            },
        })
        .await
        .unwrap();

    assert_eq!(
        ctx.runtime.count_running_agents("doctor", ""),
        1,
        "Starting agent should count"
    );

    // Add another agent
    ctx.runtime
        .executor
        .execute(oj_core::Effect::Emit {
            event: Event::AgentRunCreated {
                id: oj_core::AgentRunId::new("run-2"),
                agent_name: "doctor".to_string(),
                command_name: "test".to_string(),
                namespace: String::new(),
                cwd: ctx.project_root.clone(),
                runbook_hash: runbook_hash.clone(),
                vars: HashMap::new(),
                created_at_epoch_ms: 2000,
            },
        })
        .await
        .unwrap();

    assert_eq!(
        ctx.runtime.count_running_agents("doctor", ""),
        2,
        "Two non-terminal agents"
    );

    // Complete one
    ctx.runtime
        .executor
        .execute(oj_core::Effect::Emit {
            event: Event::AgentRunStatusChanged {
                id: oj_core::AgentRunId::new("run-1"),
                status: AgentRunStatus::Completed,
                reason: None,
            },
        })
        .await
        .unwrap();

    assert_eq!(
        ctx.runtime.count_running_agents("doctor", ""),
        1,
        "Completed agent should not count"
    );

    // Fail the other
    ctx.runtime
        .executor
        .execute(oj_core::Effect::Emit {
            event: Event::AgentRunStatusChanged {
                id: oj_core::AgentRunId::new("run-2"),
                status: AgentRunStatus::Failed,
                reason: Some("crashed".to_string()),
            },
        })
        .await
        .unwrap();

    assert_eq!(
        ctx.runtime.count_running_agents("doctor", ""),
        0,
        "Failed agent should not count"
    );
}

// ---- Test 16: count_running_agents_namespace_isolation ----

#[tokio::test]
async fn count_running_agents_namespace_isolation() {
    let ctx = setup_with_runbook(CRON_AGENT_RUNBOOK).await;
    let (runbook_json, runbook_hash) = hash_runbook(CRON_AGENT_RUNBOOK);

    load_runbook(&ctx, &runbook_json, &runbook_hash).await;

    // Add agent in namespace "ns-a"
    ctx.runtime
        .executor
        .execute(oj_core::Effect::Emit {
            event: Event::AgentRunCreated {
                id: oj_core::AgentRunId::new("run-ns-a"),
                agent_name: "doctor".to_string(),
                command_name: "test".to_string(),
                namespace: "ns-a".to_string(),
                cwd: ctx.project_root.clone(),
                runbook_hash: runbook_hash.clone(),
                vars: HashMap::new(),
                created_at_epoch_ms: 1000,
            },
        })
        .await
        .unwrap();

    // Count in namespace "ns-a" should be 1
    assert_eq!(ctx.runtime.count_running_agents("doctor", "ns-a"), 1);

    // Count in empty namespace should be 0 (different namespace)
    assert_eq!(ctx.runtime.count_running_agents("doctor", ""), 0);

    // Count in namespace "ns-b" should be 0
    assert_eq!(ctx.runtime.count_running_agents("doctor", "ns-b"), 0);
}
