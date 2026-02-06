// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use crate::{parse_runbook, parse_runbook_with_format, Format, ParseError};

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

/// Parse HCL runbook, panicking on error.
fn parse_hcl(input: &str) -> crate::Runbook {
    parse_runbook_with_format(input, Format::Hcl).unwrap()
}

/// Parse JSON runbook, panicking on error.
fn parse_json(input: &str) -> crate::Runbook {
    parse_runbook_with_format(input, Format::Json).unwrap()
}

/// Assert that a parse error's display message contains all given fragments.
fn assert_err_contains(err: &ParseError, fragments: &[&str]) {
    let msg = err.to_string();
    for frag in fragments {
        assert!(msg.contains(frag), "error should contain '{frag}': {msg}");
    }
}

/// Parse TOML and assert it fails with error containing all fragments.
fn assert_toml_err(input: &str, fragments: &[&str]) {
    assert_err_contains(&parse_runbook(input).unwrap_err(), fragments);
}

/// Parse HCL and assert it fails with error containing all fragments.
fn assert_hcl_err(input: &str, fragments: &[&str]) {
    assert_err_contains(
        &parse_runbook_with_format(input, Format::Hcl).unwrap_err(),
        fragments,
    );
}
