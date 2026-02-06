// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Parse error tests: missing fields, invalid shell, invalid structure, invalid args.

use crate::{parse_runbook, ParseError};

// ============================================================================
// Missing Required Fields
// ============================================================================

#[test]
fn missing_command_run() {
    let toml = r#"
[command.build]
args = "<name>"
"#;
    super::assert_toml_err(toml, &["run"]);
}

#[test]
fn missing_step_name() {
    let toml = r#"
[job.test]
[[job.test.step]]
run = "echo test"
"#;
    let err = parse_runbook(toml).unwrap_err();
    assert!(matches!(err, ParseError::InvalidFormat { .. }));
    super::assert_err_contains(&err, &["name"]);
}

#[test]
fn missing_step_run() {
    let toml = r#"
[job.test]
[[job.test.step]]
name = "build"
"#;
    super::assert_toml_err(toml, &["run"]);
}

// ============================================================================
// Invalid Shell Commands
// ============================================================================

#[test]
fn unterminated_quote_in_command_run() {
    let toml = r#"
[command.test]
run = "echo 'unterminated"
"#;
    let err = parse_runbook(toml).unwrap_err();
    assert!(matches!(err, ParseError::ShellError { .. }));
    super::assert_err_contains(&err, &["command.test.run"]);
}

#[test]
fn unterminated_subshell_in_step() {
    let toml = r#"
[job.test]
[[job.test.step]]
name = "broken"
run = "echo $(incomplete"
"#;
    let err = parse_runbook(toml).unwrap_err();
    assert!(matches!(err, ParseError::ShellError { .. }));
    super::assert_err_contains(&err, &["job.test.step[0](broken).run"]);
}

#[test]
fn unterminated_quote_in_agent_run() {
    let toml = r#"
[agent.test]
run = "claude \"unterminated"
"#;
    let err = parse_runbook(toml).unwrap_err();
    assert!(matches!(err, ParseError::ShellError { .. }));
    super::assert_err_contains(&err, &["agent.test.run"]);
}

// ============================================================================
// Invalid TOML Structure
// ============================================================================

#[test]
fn command_not_table() {
    let toml = r#"
[command]
build = "not a table"
"#;
    assert!(matches!(
        parse_runbook(toml).unwrap_err(),
        ParseError::Toml(_)
    ));
}

#[test]
fn invalid_run_directive() {
    let toml = r#"
[command.test]
run = { invalid = "key" }
"#;
    assert!(matches!(
        parse_runbook(toml).unwrap_err(),
        ParseError::Toml(_)
    ));
}

#[test]
fn job_not_table() {
    let toml = r#"
[job]
build = "not a table"
"#;
    assert!(matches!(
        parse_runbook(toml).unwrap_err(),
        ParseError::Toml(_)
    ));
}

// ============================================================================
// Invalid Argument Specs
// ============================================================================

#[test]
fn duplicate_arg_name() {
    let toml = r#"
[command.test]
args = "<name> <name>"
run = "test.sh"
"#;
    super::assert_toml_err(toml, &["duplicate"]);
}

#[test]
fn variadic_not_last() {
    let toml = r#"
[command.test]
args = "<files...> <extra>"
run = "test.sh"
"#;
    super::assert_toml_err(toml, &["variadic"]);
}

#[test]
fn optional_before_required() {
    let toml = r#"
[command.test]
args = "[opt] <req>"
run = "test.sh"
"#;
    assert!(parse_runbook(toml).is_err());
}

// ============================================================================
// Agent Missing Run
// ============================================================================

#[test]
fn agent_missing_run() {
    let toml = r#"
[agent.test]
prompt = "Do something"
"#;
    super::assert_toml_err(toml, &["run"]);
}
