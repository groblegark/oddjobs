// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Centralized environment variable access for the adapters crate.

use std::time::Duration;

fn parse_duration_ms(var: &str) -> Option<Duration> {
    std::env::var(var)
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map(Duration::from_millis)
}

/// Watcher fallback poll interval (default: 5000ms).
pub fn watcher_poll_ms() -> Duration {
    parse_duration_ms("OJ_WATCHER_POLL_MS").unwrap_or(Duration::from_secs(5))
}

/// Session log poll interval for `wait_for_session_log_or_exit` (default: 1000ms).
pub fn session_poll_ms() -> Duration {
    parse_duration_ms("OJ_SESSION_POLL_MS").unwrap_or(Duration::from_secs(1))
}

/// Prompt detection total poll budget (default: 3000ms).
/// Returns the number of 200ms poll attempts.
pub fn prompt_poll_max_attempts() -> usize {
    parse_duration_ms("OJ_PROMPT_POLL_MS")
        .map(|d| (d.as_millis() / 200).max(1) as usize)
        .unwrap_or(15)
}
