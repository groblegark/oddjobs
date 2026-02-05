// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use super::*;
use crate::agent::AgentError;

mod log_summary;
mod log_summary_ext;
mod serialization;
mod serialization_ext;

/// Assert that an event survives a JSON serialize/deserialize roundtrip.
fn assert_roundtrip(event: &Event) {
    let json_str = serde_json::to_string(event).unwrap();
    let parsed: Event = serde_json::from_str(&json_str).unwrap();
    assert_eq!(event, &parsed, "roundtrip failed for {:?}", event);
}
