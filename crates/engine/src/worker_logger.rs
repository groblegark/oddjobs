// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Append-only logger for per-worker activity logs.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::log_paths;
use crate::time_fmt::format_utc_now;

/// Append-only logger for per-worker activity logs.
///
/// Writes human-readable timestamped lines to:
///   `<log_dir>/worker/<worker_name>.log`
///
/// Format: `2026-01-30T08:14:09Z [worker] message`
///
/// Each `append()` call opens, writes, and closes the file.
/// This is safe for the low write frequency of worker events.
pub struct WorkerLogger {
    log_dir: PathBuf,
}

impl WorkerLogger {
    pub fn new(log_dir: PathBuf) -> Self {
        Self { log_dir }
    }

    /// Append a log line for the given worker.
    ///
    /// Format: `2026-01-30T08:14:09Z [worker] message`
    ///
    /// Failures are logged via tracing but do not propagate — logging
    /// must not break the engine.
    pub fn append(&self, worker_name: &str, message: &str) {
        let path = log_paths::worker_log_path(&self.log_dir, worker_name);
        if let Err(e) = self.write_line(&path, message) {
            tracing::warn!(
                worker_name,
                error = %e,
                "failed to write worker log"
            );
        }
    }

    fn write_line(&self, path: &Path, message: &str) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        let ts = format_utc_now();
        writeln!(file, "{} [worker] {}", ts, message)?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "worker_logger_tests.rs"]
mod tests;
