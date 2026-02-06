// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Agent configuration tests: command recognition, prompt config, session config.

use crate::{parse_runbook, parse_runbook_with_format, Format, ParseError};

// ============================================================================
// Agent Command Recognition
// ============================================================================

#[test]
fn unrecognized_agent_command() {
    let toml = r#"
[agent.test]
run = "unknown-tool -p 'do something'"
"#;
    let err = parse_runbook(toml).unwrap_err();
    assert!(matches!(err, ParseError::InvalidFormat { .. }));
    super::assert_err_contains(&err, &["unrecognized", "unknown-tool"]);
}

#[test]
fn claude_command() {
    let toml = r#"
[agent.planner]
run = "claude --print 'Plan something'"
"#;
    assert!(parse_runbook(toml).unwrap().agents.contains_key("planner"));
}

#[test]
fn claudeless_command() {
    let toml = r#"
[agent.runner]
run = "claudeless --scenario 'Run tests'"
"#;
    assert!(parse_runbook(toml).unwrap().agents.contains_key("runner"));
}

#[test]
fn absolute_path_command() {
    let toml = r#"
[agent.planner]
run = "/usr/local/bin/claude --print 'Plan something'"
"#;
    assert!(parse_runbook(toml).unwrap().agents.contains_key("planner"));
}

#[test]
fn unrecognized_absolute_path() {
    let toml = r#"
[agent.test]
run = "/usr/bin/codex --help"
"#;
    let err = parse_runbook(toml).unwrap_err();
    assert!(matches!(err, ParseError::InvalidFormat { .. }));
    super::assert_err_contains(&err, &["codex"]);
}

#[test]
fn hcl_agent_validation() {
    let hcl = r#"
agent "planner" {
  run = "claude --print 'Plan something'"
}
"#;
    assert!(super::parse_hcl(hcl).agents.contains_key("planner"));
}

#[test]
fn hcl_unrecognized_agent_command() {
    let hcl = r#"
agent "test" {
  run = "unknown-tool -p 'do something'"
}
"#;
    super::assert_hcl_err(hcl, &["unrecognized"]);
}

// ============================================================================
// Prompt Configuration
// ============================================================================

#[test]
fn prompt_field_no_inline() {
    let toml = r#"
[agent.plan]
run = "claude --dangerously-skip-permissions"
prompt = "Plan the feature"
"#;
    assert!(parse_runbook(toml).unwrap().agents.contains_key("plan"));
}

#[test]
fn prompt_file_no_inline() {
    let toml = r#"
[agent.plan]
run = "claude --dangerously-skip-permissions"
prompt_file = "prompts/plan.md"
"#;
    assert!(parse_runbook(toml).unwrap().agents.contains_key("plan"));
}

#[test]
fn prompt_field_with_positional_rejected() {
    let toml = r#"
[agent.plan]
run = "claude --print \"${prompt}\""
prompt = "Plan the feature"
"#;
    let err = parse_runbook(toml).unwrap_err();
    assert!(matches!(err, ParseError::InvalidFormat { .. }));
    super::assert_err_contains(&err, &["positional"]);
}

#[test]
fn no_prompt_no_reference() {
    let toml = r#"
[agent.plan]
run = "claude --dangerously-skip-permissions"
"#;
    assert!(parse_runbook(toml).unwrap().agents.contains_key("plan"));
}

#[test]
fn prompt_reference_without_field() {
    // ${prompt} in run without a prompt field is valid — the value comes from job input
    let toml = r#"
[agent.plan]
run = "claude -p \"${prompt}\""
"#;
    assert!(parse_runbook(toml).unwrap().agents.contains_key("plan"));
}

#[test]
fn session_id_rejected() {
    // --session-id is rejected (system adds it automatically)
    let toml = r#"
[agent.plan]
run = "claude --session-id abc123"
"#;
    let err = parse_runbook(toml).unwrap_err();
    assert!(matches!(err, ParseError::InvalidFormat { .. }));
    super::assert_err_contains(&err, &["session-id", "automatically"]);
}

#[test]
fn session_id_equals_rejected() {
    let toml = r#"
[agent.plan]
run = "claude --session-id=abc123"
"#;
    let err = parse_runbook(toml).unwrap_err();
    assert!(matches!(err, ParseError::InvalidFormat { .. }));
    super::assert_err_contains(&err, &["session-id"]);
}

// ============================================================================
// Session Configuration
// ============================================================================

#[test]
fn session_hcl_with_color() {
    let hcl = r#"
agent "mayor" {
  run = "claude"

  session "tmux" {
    color = "cyan"
    title = "mayor"
  }
}
"#;
    let runbook = super::parse_hcl(hcl);
    let tmux = runbook
        .get_agent("mayor")
        .unwrap()
        .session
        .get("tmux")
        .unwrap();
    assert_eq!(tmux.color.as_deref(), Some("cyan"));
    assert_eq!(tmux.title.as_deref(), Some("mayor"));
}

#[test]
fn session_hcl_with_status() {
    let hcl = r#"
agent "mayor" {
  run = "claude"

  session "tmux" {
    color = "green"
    status {
      left  = "myproject merge/check"
      right = "custom-id"
    }
  }
}
"#;
    let runbook = super::parse_hcl(hcl);
    let tmux = runbook
        .get_agent("mayor")
        .unwrap()
        .session
        .get("tmux")
        .unwrap();
    assert_eq!(tmux.color.as_deref(), Some("green"));
    let status = tmux.status.as_ref().unwrap();
    assert_eq!(status.left.as_deref(), Some("myproject merge/check"));
    assert_eq!(status.right.as_deref(), Some("custom-id"));
}

#[test]
fn session_hcl_no_session_block() {
    let hcl = r#"
agent "worker" {
  run = "claude"
}
"#;
    let agent = super::parse_hcl(hcl).get_agent("worker").unwrap().clone();
    assert!(agent.session.is_empty());
}

#[test]
fn session_hcl_unknown_provider() {
    let hcl = r#"
agent "worker" {
  run = "claude"

  session "zellij" {
    color = "red"
  }
}
"#;
    // Unknown providers parse without error
    let agent = super::parse_hcl(hcl).get_agent("worker").unwrap().clone();
    assert!(agent.session.contains_key("zellij"));
}

#[test]
fn session_rejects_invalid_color() {
    let hcl = r#"
agent "worker" {
  run = "claude"

  session "tmux" {
    color = "purple"
  }
}
"#;
    super::assert_hcl_err(hcl, &["unknown color 'purple'"]);
}

#[test]
fn session_accepts_all_valid_colors() {
    for color in &["red", "green", "blue", "cyan", "magenta", "yellow", "white"] {
        let hcl = format!(
            r#"
agent "worker" {{
  run = "claude"

  session "tmux" {{
    color = "{}"
  }}
}}
"#,
            color
        );
        let result = parse_runbook_with_format(&hcl, Format::Hcl);
        assert!(
            result.is_ok(),
            "color '{}' should be valid, got: {:?}",
            color,
            result.err()
        );
    }
}

#[test]
fn session_toml_roundtrip() {
    let toml = r#"
[agent.worker]
run = "claude"

[agent.worker.session.tmux]
color = "blue"
title = "my-worker"

[agent.worker.session.tmux.status]
left = "project build/execute"
right = "abc12345"
"#;
    let runbook = parse_runbook(toml).unwrap();
    let tmux = runbook
        .get_agent("worker")
        .unwrap()
        .session
        .get("tmux")
        .unwrap();
    assert_eq!(tmux.color.as_deref(), Some("blue"));
    assert_eq!(tmux.title.as_deref(), Some("my-worker"));
    let status = tmux.status.as_ref().unwrap();
    assert_eq!(status.left.as_deref(), Some("project build/execute"));
    assert_eq!(status.right.as_deref(), Some("abc12345"));
}
