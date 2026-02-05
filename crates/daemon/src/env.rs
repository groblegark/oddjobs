// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Centralized environment variable access for the daemon crate.

use std::path::PathBuf;
use std::time::Duration;

use crate::lifecycle::LifecycleError;

/// Resolve state directory: OJ_STATE_DIR > XDG_STATE_HOME/oj > ~/.local/state/oj
pub fn state_dir() -> Result<PathBuf, LifecycleError> {
    if let Ok(dir) = std::env::var("OJ_STATE_DIR") {
        return Ok(PathBuf::from(dir));
    }
    if let Ok(xdg) = std::env::var("XDG_STATE_HOME") {
        return Ok(PathBuf::from(xdg).join("oj"));
    }
    let home = std::env::var("HOME").map_err(|_| LifecycleError::NoStateDir)?;
    Ok(PathBuf::from(home).join(".local/state/oj"))
}

/// Timer check interval override
pub fn timer_check_ms() -> Option<Duration> {
    std::env::var("OJ_TIMER_CHECK_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map(Duration::from_millis)
}
