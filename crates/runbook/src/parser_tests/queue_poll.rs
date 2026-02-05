// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use crate::{parse_runbook_with_format, Format, ParseError, QueueType};

// ============================================================================
// Queue Poll Tests
// ============================================================================

#[test]
fn parse_external_queue_with_poll() {
    let hcl = r#"
queue "bugs" {
  type = "external"
  list = "wok list -t bug -o json"
  take = "wok start ${item.id}"
  poll = "30s"
}
"#;
    let runbook = parse_runbook_with_format(hcl, Format::Hcl).unwrap();
    let queue = &runbook.queues["bugs"];
    assert_eq!(queue.queue_type, QueueType::External);
    assert_eq!(queue.poll.as_deref(), Some("30s"));
}

#[test]
fn parse_external_queue_with_poll_millis() {
    let hcl = r#"
queue "fast" {
  type = "external"
  list = "echo '[]'"
  take = "echo ok"
  poll = "200ms"
}
"#;
    let runbook = parse_runbook_with_format(hcl, Format::Hcl).unwrap();
    let queue = &runbook.queues["fast"];
    assert_eq!(queue.poll.as_deref(), Some("200ms"));
}

#[test]
fn parse_external_queue_without_poll() {
    let hcl = r#"
queue "bugs" {
  type = "external"
  list = "echo '[]'"
  take = "echo ok"
}
"#;
    let runbook = parse_runbook_with_format(hcl, Format::Hcl).unwrap();
    let queue = &runbook.queues["bugs"];
    assert!(queue.poll.is_none());
}

#[test]
fn error_external_queue_with_invalid_poll() {
    let hcl = r#"
queue "bugs" {
  type = "external"
  list = "echo '[]'"
  take = "echo ok"
  poll = "bogus"
}
"#;
    let err = parse_runbook_with_format(hcl, Format::Hcl).unwrap_err();
    assert!(matches!(err, ParseError::InvalidFormat { .. }));
    let msg = err.to_string();
    assert!(
        msg.contains("queue.bugs.poll"),
        "error should mention location: {}",
        msg
    );
}

#[test]
fn error_persisted_queue_with_poll() {
    let hcl = r#"
queue "items" {
  type = "persisted"
  vars = ["branch"]
  poll = "30s"
}
"#;
    let err = parse_runbook_with_format(hcl, Format::Hcl).unwrap_err();
    assert!(matches!(err, ParseError::InvalidFormat { .. }));
    let msg = err.to_string();
    assert!(
        msg.contains("persisted queue must not have 'poll' field"),
        "error should mention forbidden poll: {}",
        msg
    );
}
