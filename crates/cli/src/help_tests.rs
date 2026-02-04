// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Tests for sectioned help output and colorization.

#![allow(clippy::unwrap_used)]

use super::*;
use crate::color::codes::{CONTEXT_START, HEADER_START, LITERAL_START, RESET};
use crate::Commands;

// ============================================================================
// Exhaustiveness Tests
// ============================================================================

/// Every subcommand registered in clap must appear in the help sections.
/// If a new subcommand is added to `Commands` but not to `help::commands()`,
/// this test fails with a clear message.
#[test]
fn all_subcommands_in_help() {
    let cmd = crate::cli_command();
    let help_text = commands();
    for sub in cmd.get_subcommands() {
        let name = sub.get_name();
        if name == "help" {
            continue; // clap auto-generated
        }
        let pattern = format!("  {}", name);
        assert!(
            help_text.contains(&pattern),
            "Command '{name}' missing from help sections — add it to help::commands()"
        );
    }
}

/// Compile-time exhaustive match on `Commands` enum.
/// Adding a new variant causes a compile error here, forcing the developer
/// to assign it to a section.
#[test]
fn all_commands_assigned_to_section() {
    fn _section(cmd: &Commands) -> &'static str {
        match cmd {
            Commands::Run(_) => "Actions",
            Commands::Status(_) => "Actions",
            Commands::Show { .. } => "Actions",
            Commands::Peek { .. } => "Actions",
            Commands::Attach { .. } => "Actions",
            Commands::Pipeline(_) => "Resources",
            Commands::Agent(_) => "Resources",
            Commands::Session(_) => "Resources",
            Commands::Workspace(_) => "Resources",
            Commands::Queue(_) => "Resources",
            Commands::Worker(_) => "Resources",
            Commands::Cron(_) => "Resources",
            Commands::Decision(_) => "Resources",
            Commands::Project(_) => "Resources",
            Commands::Env(_) => "System",
            Commands::Logs { .. } => "System",
            Commands::Emit(_) => "System",
            Commands::Daemon(_) => "System",
        }
    }
}

// ============================================================================
// Plain Text Tests
// ============================================================================

#[test]
fn commands_returns_plain_text() {
    let result = commands();
    assert!(
        !result.contains("\x1b["),
        "commands() should not contain ANSI codes"
    );
}

#[test]
fn template_returns_plain_text() {
    let result = template();
    assert!(
        !result.contains("\x1b["),
        "template() should not contain ANSI codes"
    );
}

#[test]
fn after_help_returns_plain_text() {
    let result = after_help();
    assert!(
        !result.contains("\x1b["),
        "after_help() should not contain ANSI codes"
    );
}

// ============================================================================
// Section Content Tests
// ============================================================================

#[test]
fn commands_has_actions_section() {
    let result = commands();
    assert!(result.contains("Actions:"), "Should have Actions section");
    assert!(result.contains("  run "), "Actions should contain run");
    assert!(
        result.contains("  status "),
        "Actions should contain status"
    );
    assert!(result.contains("  show "), "Actions should contain show");
    assert!(result.contains("  peek "), "Actions should contain peek");
    assert!(
        result.contains("  attach "),
        "Actions should contain attach"
    );
}

#[test]
fn commands_has_resources_section() {
    let result = commands();
    assert!(
        result.contains("Resources:"),
        "Should have Resources section"
    );
    assert!(
        result.contains("  pipeline "),
        "Resources should contain pipeline"
    );
    assert!(
        result.contains("  agent "),
        "Resources should contain agent"
    );
    assert!(
        result.contains("  session "),
        "Resources should contain session"
    );
    assert!(
        result.contains("  workspace "),
        "Resources should contain workspace"
    );
    assert!(
        result.contains("  queue "),
        "Resources should contain queue"
    );
    assert!(
        result.contains("  worker "),
        "Resources should contain worker"
    );
    assert!(result.contains("  cron "), "Resources should contain cron");
    assert!(
        result.contains("  decision "),
        "Resources should contain decision"
    );
    assert!(
        result.contains("  project "),
        "Resources should contain project"
    );
}

#[test]
fn commands_has_system_section() {
    let result = commands();
    assert!(result.contains("System:"), "Should have System section");
    assert!(result.contains("  env "), "System should contain env");
    assert!(result.contains("  logs "), "System should contain logs");
    assert!(result.contains("  emit "), "System should contain emit");
    assert!(result.contains("  daemon "), "System should contain daemon");
}

// ============================================================================
// Colorization Tests
// ============================================================================

#[test]
fn colorize_help_applies_header_color() {
    let result = colorize_help("Actions:");
    assert!(
        result.contains(&format!("{}Actions:{}", HEADER_START, RESET)),
        "Section header should be HEADER colored in:\n{}",
        result
    );
}

#[test]
fn colorize_help_applies_usage_color() {
    let result = colorize_help("Usage: oj [OPTIONS] [COMMAND]");
    assert!(
        result.contains(&format!("{}Usage:{}", HEADER_START, RESET)),
        "Usage: should be HEADER colored in:\n{}",
        result
    );
}

#[test]
fn colorize_help_applies_literal_to_commands() {
    let result = colorize_help("  run         Run a command from the runbook");
    assert!(
        result.contains(&format!("{}run{}", LITERAL_START, RESET)),
        "Command name should be LITERAL colored in:\n{}",
        result
    );
}

#[test]
fn colorize_help_applies_literal_to_option_flags() {
    let result = colorize_help("  -o, --output <OUTPUT>    Output format [default: text]");
    assert!(
        result.contains(&format!("{}-o{}", LITERAL_START, RESET)),
        "Short flag should be LITERAL colored in:\n{}",
        result
    );
    assert!(
        result.contains(&format!("{}--output{}", LITERAL_START, RESET)),
        "Long flag should be LITERAL colored in:\n{}",
        result
    );
}

#[test]
fn colorize_help_applies_context_to_placeholders() {
    let result = colorize_help("  -o, --output <OUTPUT>    Output format");
    assert!(
        result.contains(&format!("{}<OUTPUT>{}", CONTEXT_START, RESET)),
        "Placeholder should be CONTEXT colored in:\n{}",
        result
    );
}

#[test]
fn colorize_help_applies_context_to_defaults() {
    let result = colorize_help(
        "  -o, --output <OUTPUT>    Output format [default: text] [possible values: text, json]",
    );
    assert!(
        result.contains(&format!("{}[default: text]{}", CONTEXT_START, RESET)),
        "[default: text] should be CONTEXT colored in:\n{}",
        result
    );
    assert!(
        result.contains(&format!(
            "{}[possible values: text, json]{}",
            CONTEXT_START, RESET
        )),
        "[possible values: ...] should be CONTEXT colored in:\n{}",
        result
    );
}

#[test]
fn colorize_help_skips_existing_ansi() {
    let input = "\x1b[38;5;74mAlready Colored\x1b[0m";
    let result = colorize_help(input);
    assert_eq!(result, input, "Existing ANSI codes should be preserved");
}

#[test]
fn colorize_help_handles_mixed_content() {
    let input = "\
Actions:
  run         Run a command from the runbook
  status      Show overview

Options:
  -o, --output <OUTPUT>    Output format [default: text]";

    let result = colorize_help(input);

    assert!(
        result.contains(&format!("{}Actions:{}", HEADER_START, RESET)),
        "Actions header should be colored"
    );
    assert!(
        result.contains(&format!("{}Options:{}", HEADER_START, RESET)),
        "Options header should be colored"
    );
    assert!(
        result.contains(&format!("{}run{}", LITERAL_START, RESET)),
        "run command should be colored"
    );
    assert!(
        result.contains(&format!("{}--output{}", LITERAL_START, RESET)),
        "--output flag should be colored"
    );
}

// ============================================================================
// Format Help Tests
// ============================================================================

#[test]
fn format_help_produces_output() {
    let mut cmd = crate::cli_command();
    let help = format_help(&mut cmd);
    assert!(!help.is_empty(), "format_help should produce output");
    assert!(
        help.contains("Actions:") || help.contains(&format!("{}Actions:{}", HEADER_START, RESET)),
        "Help should contain Actions section"
    );
}

#[test]
fn format_help_ends_with_newline() {
    let mut cmd = crate::cli_command();
    let help = format_help(&mut cmd);
    assert!(help.ends_with('\n'), "Help should end with newline");
}
