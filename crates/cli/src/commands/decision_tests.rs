// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use clap::Parser;

use super::*;

/// Wrapper for testing DecisionCommand parsing
#[derive(Parser)]
struct TestCli {
    #[command(subcommand)]
    command: DecisionCommand,
}

#[test]
fn parse_list() {
    let cli = TestCli::parse_from(["test", "list"]);
    assert!(matches!(
        cli.command,
        DecisionCommand::List { project: None }
    ));
}

#[test]
fn parse_list_with_project() {
    let cli = TestCli::parse_from(["test", "list", "--project", "myproject"]);
    if let DecisionCommand::List { project } = cli.command {
        assert_eq!(project, Some("myproject".to_string()));
    } else {
        panic!("expected List");
    }
}

#[test]
fn parse_show() {
    let cli = TestCli::parse_from(["test", "show", "abc123"]);
    if let DecisionCommand::Show { id } = cli.command {
        assert_eq!(id, "abc123");
    } else {
        panic!("expected Show");
    }
}

#[test]
fn parse_resolve_with_choice() {
    let cli = TestCli::parse_from(["test", "resolve", "abc123", "2"]);
    if let DecisionCommand::Resolve {
        id,
        choice,
        message,
    } = cli.command
    {
        assert_eq!(id, "abc123");
        assert_eq!(choice, Some(2));
        assert_eq!(message, None);
    } else {
        panic!("expected Resolve");
    }
}

#[test]
fn parse_resolve_with_message() {
    let cli = TestCli::parse_from(["test", "resolve", "abc123", "-m", "looks good"]);
    if let DecisionCommand::Resolve {
        id,
        choice,
        message,
    } = cli.command
    {
        assert_eq!(id, "abc123");
        assert_eq!(choice, None);
        assert_eq!(message, Some("looks good".to_string()));
    } else {
        panic!("expected Resolve");
    }
}

#[test]
fn parse_resolve_with_choice_and_message() {
    let cli = TestCli::parse_from(["test", "resolve", "abc123", "1", "-m", "approved"]);
    if let DecisionCommand::Resolve {
        id,
        choice,
        message,
    } = cli.command
    {
        assert_eq!(id, "abc123");
        assert_eq!(choice, Some(1));
        assert_eq!(message, Some("approved".to_string()));
    } else {
        panic!("expected Resolve");
    }
}
