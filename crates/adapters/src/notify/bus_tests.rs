// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use super::*;

fn assert_clone<T: Clone>() {}
fn assert_send<T: Send>() {}
fn assert_sync<T: Sync>() {}

#[test]
fn bus_notify_adapter_is_clone_send_sync() {
    assert_clone::<BusNotifyAdapter>();
    assert_send::<BusNotifyAdapter>();
    assert_sync::<BusNotifyAdapter>();
}

#[test]
fn bus_notify_adapter_new_does_not_panic() {
    let _adapter = BusNotifyAdapter::new();
}

#[tokio::test]
async fn bus_notify_returns_ok_when_disabled() {
    // OJ_BUS_EMIT is not set in the test environment, so adapter is disabled.
    let adapter = BusNotifyAdapter::new();
    let result = adapter.notify("test title", "test message").await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn bus_notify_returns_ok_when_bd_missing() {
    // Force enabled, but bd binary won't exist — should still return Ok
    // (fire-and-forget: errors are logged, not returned).
    let adapter = BusNotifyAdapter { enabled: true };
    let result = adapter.notify("test title", "test message").await;
    assert!(result.is_ok());
}
