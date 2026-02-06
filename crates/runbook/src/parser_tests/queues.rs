// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Queue type and poll configuration tests.

use crate::QueueType;

// ============================================================================
// Queue Type Tests
// ============================================================================

#[test]
fn external_queue_with_explicit_type() {
    let hcl = r#"
queue "bugs" {
  type = "external"
  list = "wok list -t bug -o json"
  take = "wok start ${item.id}"
}
"#;
    let runbook = super::parse_hcl(hcl);
    let queue = &runbook.queues["bugs"];
    assert_eq!(queue.queue_type, QueueType::External);
    assert_eq!(queue.list.as_deref(), Some("wok list -t bug -o json"));
    assert_eq!(queue.take.as_deref(), Some("wok start ${item.id}"));
}

#[test]
fn persisted_queue() {
    let hcl = r#"
queue "merges" {
  type     = "persisted"
  vars     = ["branch", "title", "base"]
  defaults = { base = "main" }
}
"#;
    let runbook = super::parse_hcl(hcl);
    let queue = &runbook.queues["merges"];
    assert_eq!(queue.queue_type, QueueType::Persisted);
    assert_eq!(queue.vars, vec!["branch", "title", "base"]);
    assert_eq!(queue.defaults.get("base"), Some(&"main".to_string()));
    assert!(queue.list.is_none());
    assert!(queue.take.is_none());
}

#[test]
fn queue_defaults_to_external() {
    let hcl = r#"
queue "items" {
  list = "echo '[]'"
  take = "echo ok"
}
"#;
    let runbook = super::parse_hcl(hcl);
    assert_eq!(runbook.queues["items"].queue_type, QueueType::External);
}

#[test]
fn error_external_missing_list() {
    let hcl = r#"
queue "items" {
  type = "external"
  take = "echo ok"
}
"#;
    super::assert_hcl_err(hcl, &["external queue requires 'list' field"]);
}

#[test]
fn error_external_missing_take() {
    let hcl = r#"
queue "items" {
  type = "external"
  list = "echo '[]'"
}
"#;
    super::assert_hcl_err(hcl, &["external queue requires 'take' field"]);
}

#[test]
fn error_persisted_missing_vars() {
    let hcl = r#"
queue "items" {
  type = "persisted"
}
"#;
    super::assert_hcl_err(hcl, &["persisted queue requires 'vars' field"]);
}

#[test]
fn error_persisted_with_list() {
    let hcl = r#"
queue "items" {
  type = "persisted"
  vars = ["branch"]
  list = "echo '[]'"
}
"#;
    super::assert_hcl_err(hcl, &["persisted queue must not have 'list' field"]);
}

#[test]
fn error_persisted_with_take() {
    let hcl = r#"
queue "items" {
  type = "persisted"
  vars = ["branch"]
  take = "echo ok"
}
"#;
    super::assert_hcl_err(hcl, &["persisted queue must not have 'take' field"]);
}

// ============================================================================
// Queue Poll Tests
// ============================================================================

#[test]
fn external_queue_with_poll() {
    let hcl = r#"
queue "bugs" {
  type = "external"
  list = "wok list -t bug -o json"
  take = "wok start ${item.id}"
  poll = "30s"
}
"#;
    let runbook = super::parse_hcl(hcl);
    let queue = &runbook.queues["bugs"];
    assert_eq!(queue.queue_type, QueueType::External);
    assert_eq!(queue.poll.as_deref(), Some("30s"));
}

#[test]
fn external_queue_with_poll_millis() {
    let hcl = r#"
queue "fast" {
  type = "external"
  list = "echo '[]'"
  take = "echo ok"
  poll = "200ms"
}
"#;
    let runbook = super::parse_hcl(hcl);
    assert_eq!(runbook.queues["fast"].poll.as_deref(), Some("200ms"));
}

#[test]
fn external_queue_without_poll() {
    let hcl = r#"
queue "bugs" {
  type = "external"
  list = "echo '[]'"
  take = "echo ok"
}
"#;
    assert!(super::parse_hcl(hcl).queues["bugs"].poll.is_none());
}

#[test]
fn error_external_with_invalid_poll() {
    let hcl = r#"
queue "bugs" {
  type = "external"
  list = "echo '[]'"
  take = "echo ok"
  poll = "bogus"
}
"#;
    super::assert_hcl_err(hcl, &["queue.bugs.poll"]);
}

#[test]
fn error_persisted_with_poll() {
    let hcl = r#"
queue "items" {
  type = "persisted"
  vars = ["branch"]
  poll = "30s"
}
"#;
    super::assert_hcl_err(hcl, &["persisted queue must not have 'poll' field"]);
}
