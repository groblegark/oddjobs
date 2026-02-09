// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Event bus notification adapter.
//!
//! Emits notifications as events on the beads event bus by spawning
//! `bd bus emit` as a subprocess. Gated behind the `OJ_BUS_EMIT` env var:
//! when not set to `"1"`, all calls are a silent no-op.

use super::{NotifyAdapter, NotifyError};
use async_trait::async_trait;

#[derive(Clone, Copy, Debug)]
pub struct BusNotifyAdapter {
    enabled: bool,
}

impl Default for BusNotifyAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl BusNotifyAdapter {
    pub fn new() -> Self {
        let enabled = std::env::var("OJ_BUS_EMIT")
            .map(|v| v == "1")
            .unwrap_or(false);
        Self { enabled }
    }
}

#[async_trait]
impl NotifyAdapter for BusNotifyAdapter {
    async fn notify(&self, title: &str, message: &str) -> Result<(), NotifyError> {
        if !self.enabled {
            return Ok(());
        }

        let payload =
            serde_json::json!({ "title": title, "message": message }).to_string();

        tokio::spawn(async move {
            tracing::info!(%payload, "emitting OjNotification via bd bus emit");
            match tokio::process::Command::new("bd")
                .arg("bus")
                .arg("emit")
                .arg("--event=OjNotification")
                .arg(format!("--payload={payload}"))
                .output()
                .await
            {
                Ok(output) if output.status.success() => {
                    tracing::info!("bd bus emit succeeded");
                }
                Ok(output) => {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    tracing::warn!(%stderr, "bd bus emit exited with non-zero status");
                }
                Err(e) => {
                    tracing::warn!(error = %e, "bd bus emit failed to spawn");
                }
            }
        });

        Ok(())
    }
}

#[cfg(test)]
#[path = "bus_tests.rs"]
mod tests;
