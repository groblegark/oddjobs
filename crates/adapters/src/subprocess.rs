// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Subprocess execution helpers

use std::process::Output;
use std::time::Duration;
use tokio::process::Command;

/// Default timeout for tmux commands.
pub const TMUX_TIMEOUT: Duration = Duration::from_secs(10);

/// Default timeout for git worktree operations.
pub const GIT_WORKTREE_TIMEOUT: Duration = Duration::from_secs(60);

/// Default timeout for shell evaluation commands.
pub const SHELL_EVAL_TIMEOUT: Duration = Duration::from_secs(10);

/// Run a subprocess command with a timeout.
///
/// Wraps `Command::output()` with `tokio::time::timeout`, converting
/// timeout expiration into a descriptive error message. The child process
/// is killed automatically if the timeout elapses (via the tokio `Child`
/// drop implementation).
pub async fn run_with_timeout(
    mut cmd: Command,
    timeout: Duration,
    description: &str,
) -> Result<Output, String> {
    match tokio::time::timeout(timeout, cmd.output()).await {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(io_err)) => Err(format!("{} failed: {}", description, io_err)),
        Err(_elapsed) => Err(format!(
            "{} timed out after {}s",
            description,
            timeout.as_secs()
        )),
    }
}

#[cfg(test)]
#[path = "subprocess_tests.rs"]
mod tests;
