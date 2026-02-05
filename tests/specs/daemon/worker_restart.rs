//! Worker restart with in-flight items specs
//!
//! Verify that workers correctly handle in-flight items across restarts,
//! and that item state is properly reconciled after daemon crashes.

use crate::prelude::*;

/// Queue-driven shell job runbook with a slow command.
/// The slow command gives us time to stop the worker mid-job.
const SLOW_SHELL_RUNBOOK: &str = r#"
[queue.tasks]
type = "persisted"
vars = ["cmd"]

[worker.runner]
source = { queue = "tasks" }
handler = { job = "process" }
concurrency = 1

[job.process]
vars = ["cmd"]

[[job.process.step]]
name = "work"
run = "${item.cmd}"
"#;

/// Scenario for an agent that runs a slow command.
/// The sleep gives us time to kill the daemon mid-job.
const SLOW_AGENT_SCENARIO: &str = r#"
name = "slow-agent"
trusted = true

[[responses]]
pattern = { type = "any" }

[responses.response]
text = "Running a slow task..."

[[responses.response.tool_calls]]
tool = "Bash"
input = { command = "sleep 2" }

[tool_execution]
mode = "live"

[tool_execution.tools.Bash]
auto_approve = true
"#;

/// Queue-driven agent job for crash recovery testing.
/// Worker takes queue items and runs an agent that sleeps.
/// on_dead = "done" advances the job when the agent exits after crash.
fn crash_recovery_queue_runbook(scenario_path: &std::path::Path) -> String {
    format!(
        r#"
[queue.tasks]
type = "persisted"
vars = ["name"]

[worker.runner]
source = {{ queue = "tasks" }}
handler = {{ job = "process" }}
concurrency = 1

[job.process]
vars = ["name"]

[[job.process.step]]
name = "work"
run = {{ agent = "slow" }}

[agent.slow]
run = "claudeless --scenario {} -p"
prompt = "Run a slow task."
on_dead = "done"
"#,
        scenario_path.display()
    )
}

// =============================================================================
// Test 1: Worker stop while job running → job completes → item released
// =============================================================================

/// When a worker is stopped while a job is running, the job should continue
/// to completion, and the queue item should be released (completed) correctly.
#[test]
fn worker_stop_while_job_running_completes_item() {
    let temp = Project::empty();
    temp.git_init();
    temp.file(".oj/runbooks/queue.toml", SLOW_SHELL_RUNBOOK);

    temp.oj().args(&["daemon", "start"]).passes();
    temp.oj().args(&["worker", "start", "runner"]).passes();

    // Push item with a slow command so we can stop the worker mid-job
    temp.oj()
        .args(&[
            "queue",
            "push",
            "tasks",
            r#"{"cmd": "sleep 1 && echo done"}"#,
        ])
        .passes();

    // Wait for the queue item to become active (job is running)
    let active = wait_for(SPEC_WAIT_MAX_MS, || {
        let items = temp
            .oj()
            .args(&["queue", "show", "tasks"])
            .passes()
            .stdout();
        items.contains("active")
    });
    assert!(active, "queue item should become active");

    // Stop the worker while job is running
    temp.oj().args(&["worker", "stop", "runner"]).passes();

    // Job should continue to completion even though worker is stopped
    let completed = wait_for(SPEC_WAIT_MAX_MS * 3, || {
        let items = temp
            .oj()
            .args(&["queue", "show", "tasks"])
            .passes()
            .stdout();
        items.contains("completed")
    });

    if !completed {
        eprintln!("=== DAEMON LOG ===\n{}\n=== END LOG ===", temp.daemon_log());
        eprintln!(
            "=== QUEUE ITEMS ===\n{}\n=== END ITEMS ===",
            temp.oj()
                .args(&["queue", "show", "tasks"])
                .passes()
                .stdout()
        );
        eprintln!(
            "=== JOBS ===\n{}\n=== END JOBS ===",
            temp.oj().args(&["job", "list"]).passes().stdout()
        );
    }
    assert!(
        completed,
        "queue item should complete even after worker stop"
    );
}

// =============================================================================
// Test 2: Worker stop → restart → processes new items
// =============================================================================

/// After stopping and restarting a worker, it should resume processing
/// new queue items.
#[test]
fn worker_restart_processes_new_items() {
    let temp = Project::empty();
    temp.git_init();
    temp.file(".oj/runbooks/queue.toml", SLOW_SHELL_RUNBOOK);

    temp.oj().args(&["daemon", "start"]).passes();
    temp.oj().args(&["worker", "start", "runner"]).passes();

    // Complete one item to verify worker is functional
    temp.oj()
        .args(&["queue", "push", "tasks", r#"{"cmd": "echo first"}"#])
        .passes();

    let first_done = wait_for(SPEC_WAIT_MAX_MS, || {
        let items = temp
            .oj()
            .args(&["queue", "show", "tasks"])
            .passes()
            .stdout();
        items.contains("completed")
    });
    assert!(first_done, "first item should complete before stop");

    // Stop the worker
    temp.oj().args(&["worker", "stop", "runner"]).passes();

    // Push a new item while worker is stopped
    temp.oj()
        .args(&["queue", "push", "tasks", r#"{"cmd": "echo second"}"#])
        .passes();

    // Verify item is pending (worker is stopped)
    let pending = wait_for(SPEC_WAIT_MAX_MS, || {
        let items = temp
            .oj()
            .args(&["queue", "show", "tasks"])
            .passes()
            .stdout();
        items.contains("pending")
    });
    assert!(pending, "new item should be pending while worker stopped");

    // Restart the worker
    temp.oj().args(&["worker", "start", "runner"]).passes();

    // Worker should process the pending item
    let second_done = wait_for(SPEC_WAIT_MAX_MS, || {
        let items = temp
            .oj()
            .args(&["queue", "show", "tasks"])
            .passes()
            .stdout();
        items.matches("completed").count() >= 2
    });

    if !second_done {
        eprintln!("=== DAEMON LOG ===\n{}\n=== END LOG ===", temp.daemon_log());
    }
    assert!(second_done, "worker should process new items after restart");
}

// =============================================================================
// Test 3: Daemon crash with active item → restart → item state reconciled
// =============================================================================

/// When the daemon crashes while a queue item's job is running an agent,
/// restarting the daemon triggers reconciliation which detects the dead agent,
/// fires on_dead = "done" to advance the job, and the worker marks the
/// queue item as completed.
#[test]
fn daemon_crash_with_active_item_reconciles_state() {
    let temp = Project::empty();
    temp.git_init();

    // Set up scenario and runbook
    temp.file(".oj/scenarios/slow.toml", SLOW_AGENT_SCENARIO);
    let scenario_path = temp.path().join(".oj/scenarios/slow.toml");
    temp.file(
        ".oj/runbooks/queue.toml",
        &crash_recovery_queue_runbook(&scenario_path),
    );

    temp.oj().args(&["daemon", "start"]).passes();
    temp.oj().args(&["worker", "start", "runner"]).passes();

    // Push item — the agent will start running a slow task
    temp.oj()
        .args(&["queue", "push", "tasks", r#"{"name": "crash-test"}"#])
        .passes();

    // Wait for the queue item to become active and the job to reach running
    let active = wait_for(SPEC_WAIT_MAX_MS, || {
        let items = temp
            .oj()
            .args(&["queue", "show", "tasks"])
            .passes()
            .stdout();
        let jobs = temp.oj().args(&["job", "list"]).passes().stdout();
        items.contains("active") && jobs.contains("running")
    });
    assert!(active, "queue item should be active with a running job");

    // Kill the daemon with SIGKILL (simulates crash)
    let killed = temp.daemon_kill();
    assert!(killed, "should be able to kill daemon");

    // Wait for daemon to actually die
    let daemon_dead = wait_for(SPEC_WAIT_MAX_MS, || {
        let output = temp
            .oj()
            .args(&["daemon", "status"])
            .command()
            .output()
            .expect("command should run");
        let stdout = String::from_utf8_lossy(&output.stdout);
        !stdout.contains("Status: running")
    });
    assert!(daemon_dead, "daemon should be dead after kill");

    // Restart the daemon — triggers reconciliation
    temp.oj().args(&["daemon", "start"]).passes();

    // Wait for the job to complete via recovery (on_dead = "done")
    // and the queue item to reach completed status
    let item_completed = wait_for(SPEC_WAIT_MAX_MS * 3, || {
        let items = temp
            .oj()
            .args(&["queue", "show", "tasks"])
            .passes()
            .stdout();
        items.contains("completed")
    });

    if !item_completed {
        eprintln!("=== DAEMON LOG ===\n{}\n=== END LOG ===", temp.daemon_log());
        eprintln!(
            "=== QUEUE ITEMS ===\n{}\n=== END ITEMS ===",
            temp.oj()
                .args(&["queue", "show", "tasks"])
                .passes()
                .stdout()
        );
        eprintln!(
            "=== JOBS ===\n{}\n=== END JOBS ===",
            temp.oj().args(&["job", "list"]).passes().stdout()
        );
    }
    assert!(
        item_completed,
        "queue item should complete after daemon crash recovery"
    );
}

// =============================================================================
// Test 4: Worker stop with multiple in-flight items → all complete
// =============================================================================

/// When a worker is stopped with multiple jobs running, all in-flight jobs
/// should complete and their queue items should be released correctly.
#[test]
fn worker_stop_with_multiple_inflight_items_completes_all() {
    let temp = Project::empty();
    temp.git_init();

    // Use a runbook with higher concurrency
    let multi_worker_runbook = r#"
[queue.tasks]
type = "persisted"
vars = ["cmd"]

[worker.runner]
source = { queue = "tasks" }
handler = { job = "process" }
concurrency = 3

[job.process]
vars = ["cmd"]

[[job.process.step]]
name = "work"
run = "${item.cmd}"
"#;
    temp.file(".oj/runbooks/queue.toml", multi_worker_runbook);

    temp.oj().args(&["daemon", "start"]).passes();
    temp.oj().args(&["worker", "start", "runner"]).passes();

    // Push multiple items with slow commands
    for i in 1..=3 {
        temp.oj()
            .args(&[
                "queue",
                "push",
                "tasks",
                &format!(r#"{{"cmd": "sleep 1 && echo item{}"}}"#, i),
            ])
            .passes();
    }

    // Wait for all items to become active
    let all_active = wait_for(SPEC_WAIT_MAX_MS, || {
        let items = temp
            .oj()
            .args(&["queue", "show", "tasks"])
            .passes()
            .stdout();
        items.matches("active").count() >= 3
    });
    assert!(all_active, "all items should become active");

    // Stop the worker while jobs are running
    temp.oj().args(&["worker", "stop", "runner"]).passes();

    // All jobs should continue to completion
    let all_completed = wait_for(SPEC_WAIT_MAX_MS * 3, || {
        let items = temp
            .oj()
            .args(&["queue", "show", "tasks"])
            .passes()
            .stdout();
        items.matches("completed").count() >= 3
    });

    if !all_completed {
        eprintln!("=== DAEMON LOG ===\n{}\n=== END LOG ===", temp.daemon_log());
        eprintln!(
            "=== QUEUE ITEMS ===\n{}\n=== END ITEMS ===",
            temp.oj()
                .args(&["queue", "show", "tasks"])
                .passes()
                .stdout()
        );
    }
    assert!(
        all_completed,
        "all in-flight items should complete after worker stop"
    );
}
