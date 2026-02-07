// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Runbook materialization at daemon startup.
//!
//! Runs `bd runbook materialize --all --force` to ensure all bead-defined
//! runbooks are materialized to disk before the daemon starts processing
//! work. Failures are logged but do not block startup.

use std::time::Duration;

use oj_adapters::subprocess::run_with_timeout;
use tokio::process::Command;
use tracing::{info, warn};

/// Timeout for runbook materialization (5 minutes).
const MATERIALIZE_TIMEOUT: Duration = Duration::from_secs(300);

/// Materialize all runbooks from bead definitions.
///
/// Spawns `bd runbook materialize --all --force` as a subprocess. On success,
/// logs the result. On failure or timeout, logs a warning and returns —
/// materialization is best-effort and must not block daemon startup.
pub(crate) async fn materialize_runbooks() {
    info!("materializing runbooks from bead definitions");

    let mut cmd = Command::new("bd");
    cmd.args(["runbook", "materialize", "--all", "--force"]);

    match run_with_timeout(cmd, MATERIALIZE_TIMEOUT, "bd runbook materialize").await {
        Ok(output) => {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                info!(
                    "runbook materialization complete{}",
                    if stdout.trim().is_empty() {
                        String::new()
                    } else {
                        format!(": {}", stdout.trim())
                    }
                );
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                warn!(
                    exit_code = output.status.code(),
                    stderr = %stderr.trim(),
                    "runbook materialization failed (non-zero exit), continuing startup"
                );
            }
        }
        Err(e) => {
            warn!(
                error = %e,
                "runbook materialization failed, continuing startup"
            );
        }
    }
}
