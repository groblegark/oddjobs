// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Integration tests for runbook parsing across TOML, JSON, and HCL formats.

#![allow(clippy::unwrap_used, clippy::panic)]

use oj_runbook::{parse_runbook, parse_runbook_with_format, Format, ParseError, Runbook};

mod action_trigger;
mod agents;
mod cron;
mod epic;
mod errors;
mod formats;
mod prime;
mod queues;
mod references;
mod template_refs;

// ---------------------------------------------------------------------------
// Shared test helpers
// ---------------------------------------------------------------------------

fn parse_hcl(input: &str) -> Runbook {
    parse_runbook_with_format(input, Format::Hcl).unwrap()
}

fn parse_json(input: &str) -> Runbook {
    parse_runbook_with_format(input, Format::Json).unwrap()
}

fn assert_err_contains(err: &ParseError, fragments: &[&str]) {
    let msg = err.to_string();
    for frag in fragments {
        assert!(msg.contains(frag), "error should contain '{frag}': {msg}");
    }
}

fn assert_toml_err(input: &str, fragments: &[&str]) {
    assert_err_contains(&parse_runbook(input).unwrap_err(), fragments);
}

fn assert_hcl_err(input: &str, fragments: &[&str]) {
    assert_err_contains(
        &parse_runbook_with_format(input, Format::Hcl).unwrap_err(),
        fragments,
    );
}
