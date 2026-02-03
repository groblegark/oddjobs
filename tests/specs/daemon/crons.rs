//! Cron scheduling e2e tests
//!
//! Verifies that crons start, fire on their interval, and stop correctly
//! using the real daemon with real wall-clock timers.

use crate::prelude::*;

/// Runbook with a cron that fires every 2 seconds and runs a simple shell pipeline.
const FAST_CRON_RUNBOOK: &str = r#"
[cron.ticker]
interval = "2s"
run = { pipeline = "tick" }

[pipeline.tick]

[[pipeline.tick.step]]
name = "work"
run = "echo tick"
"#;

/// Verifies the full cron lifecycle: start → fire → pipeline created → stop.
///
/// Uses a 2-second interval so the cron fires quickly. The test:
/// 1. Starts the daemon and the cron via `oj cron start`
/// 2. Verifies the cron appears as running in `oj cron list`
/// 3. Waits for the cron timer to fire and create a pipeline
/// 4. Stops the cron and verifies it is no longer running
#[test]
fn cron_start_fires_and_creates_pipeline() {
    let temp = Project::empty();
    temp.git_init();
    temp.file(".oj/runbooks/cron.toml", FAST_CRON_RUNBOOK);

    temp.oj().args(&["daemon", "start"]).passes();

    // Start the cron
    temp.oj()
        .args(&["cron", "start", "ticker"])
        .passes()
        .stdout_has("Cron 'ticker' started");

    // Verify cron appears in list as running
    temp.oj()
        .args(&["cron", "list"])
        .passes()
        .stdout_has("ticker")
        .stdout_has("running");

    // Wait for the cron to fire and create a pipeline (interval is 2s)
    let fired = wait_for(SPEC_WAIT_MAX_MS * 5, || {
        let output = temp.oj().args(&["pipeline", "list"]).passes().stdout();
        output.contains("tick")
    });

    if !fired {
        eprintln!("=== DAEMON LOG ===\n{}\n=== END LOG ===", temp.daemon_log());
    }
    assert!(
        fired,
        "cron should fire and create a pipeline within the wait period"
    );

    // Stop the cron
    temp.oj()
        .args(&["cron", "stop", "ticker"])
        .passes()
        .stdout_has("Cron 'ticker' stopped");

    // Verify cron is now stopped
    temp.oj()
        .args(&["cron", "list"])
        .passes()
        .stdout_has("stopped");
}

/// Verifies that `oj cron once` runs the pipeline immediately without waiting
/// for the interval timer.
#[test]
fn cron_once_runs_immediately() {
    let temp = Project::empty();
    temp.git_init();
    temp.file(".oj/runbooks/cron.toml", FAST_CRON_RUNBOOK);

    temp.oj().args(&["daemon", "start"]).passes();

    // Run the cron's pipeline once
    temp.oj()
        .args(&["cron", "once", "ticker"])
        .passes()
        .stdout_has("Pipeline")
        .stdout_has("started");

    // Pipeline should appear quickly (no 2s interval wait)
    let created = wait_for(SPEC_WAIT_MAX_MS, || {
        let output = temp.oj().args(&["pipeline", "list"]).passes().stdout();
        output.contains("tick")
    });

    assert!(created, "cron once should create a pipeline immediately");
}
