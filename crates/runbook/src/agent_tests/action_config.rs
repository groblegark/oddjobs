// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use super::super::*;

// =============================================================================
// Action Configuration Tests
// =============================================================================

#[test]
fn parses_simple_action() {
    #[derive(Debug, Deserialize)]
    struct TestConfig {
        #[serde(default)]
        on_idle: ActionConfig,
        #[serde(default = "default_on_dead")]
        on_dead: ActionConfig,
    }

    let toml = r#"
        on_idle = "nudge"
        on_dead = "escalate"
    "#;
    let config: TestConfig = toml::from_str(toml).unwrap();
    assert_eq!(config.on_idle.action(), &AgentAction::Nudge);
    assert_eq!(config.on_dead.action(), &AgentAction::Escalate);
}

#[test]
fn parses_action_with_message() {
    #[derive(Debug, Deserialize)]
    struct TestConfig {
        #[serde(default)]
        on_idle: ActionConfig,
    }

    let toml = r#"
        on_idle = { action = "nudge", message = "Keep going" }
    "#;
    let config: TestConfig = toml::from_str(toml).unwrap();
    assert_eq!(config.on_idle.action(), &AgentAction::Nudge);
    assert_eq!(config.on_idle.message(), Some("Keep going"));
    assert!(!config.on_idle.append());
}

#[test]
fn parses_action_with_append() {
    #[derive(Debug, Deserialize)]
    struct TestConfig {
        #[serde(default = "default_on_dead")]
        on_dead: ActionConfig,
    }

    let toml = r#"
        on_dead = { action = "resume", message = "Previous attempt exited.", append = true }
    "#;
    let config: TestConfig = toml::from_str(toml).unwrap();
    assert_eq!(config.on_dead.action(), &AgentAction::Resume);
    assert_eq!(config.on_dead.message(), Some("Previous attempt exited."));
    assert!(config.on_dead.append());
}

#[test]
fn parses_per_error_actions() {
    #[derive(Debug, Deserialize)]
    struct TestConfig {
        #[serde(default = "default_on_error")]
        on_error: ErrorActionConfig,
    }

    let toml = r#"
        [[on_error]]
        match = "no_internet"
        action = "resume"
        message = "Network restored"

        [[on_error]]
        action = "escalate"
    "#;
    let config: TestConfig = toml::from_str(toml).unwrap();

    // Match specific error type
    let action = config.on_error.action_for(Some(&ErrorType::NoInternet));
    assert_eq!(action.action(), &AgentAction::Resume);
    assert_eq!(action.message(), Some("Network restored"));

    // Fall through to catch-all
    let action = config.on_error.action_for(Some(&ErrorType::Unauthorized));
    assert_eq!(action.action(), &AgentAction::Escalate);
}

#[test]
fn error_action_config_simple() {
    #[derive(Debug, Deserialize)]
    struct TestConfig {
        #[serde(default = "default_on_error")]
        on_error: ErrorActionConfig,
    }

    let toml = r#"
        on_error = "escalate"
    "#;
    let config: TestConfig = toml::from_str(toml).unwrap();

    let action = config.on_error.action_for(Some(&ErrorType::NoInternet));
    assert_eq!(action.action(), &AgentAction::Escalate);
}

#[test]
fn error_action_config_default_when_no_match() {
    #[derive(Debug, Deserialize)]
    struct TestConfig {
        #[serde(default = "default_on_error")]
        on_error: ErrorActionConfig,
    }

    // Only matches rate_limited, no catch-all
    let toml = r#"
        [[on_error]]
        match = "rate_limited"
        action = "resume"
    "#;
    let config: TestConfig = toml::from_str(toml).unwrap();

    // Should default to escalate when no match
    let action = config.on_error.action_for(Some(&ErrorType::NoInternet));
    assert_eq!(action.action(), &AgentAction::Escalate);
}

#[test]
fn action_config_defaults() {
    // Defaults: on_idle = "escalate", on_dead = "escalate", on_error = "escalate"
    let default_idle = ActionConfig::default();
    assert_eq!(default_idle.action(), &AgentAction::Escalate);

    let default_exit = default_on_dead();
    assert_eq!(default_exit.action(), &AgentAction::Escalate);

    let default_error = default_on_error();
    let action = default_error.action_for(Some(&ErrorType::Unauthorized));
    assert_eq!(action.action(), &AgentAction::Escalate);
}

#[test]
fn parses_full_agent_with_actions() {
    let toml = r#"
        name = "worker"
        run = "claude -p"
        prompt = "Do the task."
        on_idle = { action = "nudge", message = "Keep going" }
        on_dead = "escalate"
        on_error = "escalate"
    "#;
    let agent: AgentDef = toml::from_str(toml).unwrap();
    assert_eq!(agent.on_idle.action(), &AgentAction::Nudge);
    assert_eq!(agent.on_idle.message(), Some("Keep going"));
    assert_eq!(agent.on_dead.action(), &AgentAction::Escalate);
}

// =============================================================================
// Attempts Parsing Tests
// =============================================================================

#[test]
fn attempts_default_is_one() {
    assert_eq!(Attempts::default(), Attempts::Finite(1));
}

#[test]
fn attempts_finite_parses() {
    let toml = r#"
        on_idle = { action = "nudge", attempts = 3 }
    "#;
    #[derive(Debug, Deserialize)]
    struct TestConfig {
        #[serde(default)]
        on_idle: ActionConfig,
    }
    let config: TestConfig = toml::from_str(toml).unwrap();
    assert_eq!(config.on_idle.attempts(), Attempts::Finite(3));
}

#[test]
fn attempts_forever_parses() {
    let toml = r#"
        on_idle = { action = "nudge", attempts = "forever" }
    "#;
    #[derive(Debug, Deserialize)]
    struct TestConfig {
        #[serde(default)]
        on_idle: ActionConfig,
    }
    let config: TestConfig = toml::from_str(toml).unwrap();
    assert_eq!(config.on_idle.attempts(), Attempts::Forever);
}

#[test]
fn attempts_zero_fails() {
    let toml = r#"
        on_idle = { action = "nudge", attempts = 0 }
    "#;
    #[derive(Debug, Deserialize)]
    // NOTE(lifetime): serde target for negative deserialization test
    #[allow(dead_code)]
    struct TestConfig {
        #[serde(default)]
        on_idle: ActionConfig,
    }
    let result: Result<TestConfig, _> = toml::from_str(toml);
    assert!(result.is_err(), "expected attempts = 0 to fail parsing");
}

#[test]
fn attempts_invalid_string_fails() {
    let toml = r#"
        on_idle = { action = "nudge", attempts = "infinite" }
    "#;
    #[derive(Debug, Deserialize)]
    // NOTE(lifetime): serde target for negative deserialization test
    #[allow(dead_code)]
    struct TestConfig {
        #[serde(default)]
        on_idle: ActionConfig,
    }
    let result: Result<TestConfig, _> = toml::from_str(toml);
    assert!(result.is_err());
}

#[test]
fn attempts_missing_defaults_to_one() {
    let toml = r#"
        on_idle = { action = "nudge" }
    "#;
    #[derive(Debug, Deserialize)]
    struct TestConfig {
        #[serde(default)]
        on_idle: ActionConfig,
    }
    let config: TestConfig = toml::from_str(toml).unwrap();
    assert_eq!(config.on_idle.attempts(), Attempts::Finite(1));
}

#[test]
fn attempts_simple_action_returns_default() {
    let toml = r#"
        on_idle = "nudge"
    "#;
    #[derive(Debug, Deserialize)]
    struct TestConfig {
        #[serde(default)]
        on_idle: ActionConfig,
    }
    let config: TestConfig = toml::from_str(toml).unwrap();
    assert_eq!(config.on_idle.attempts(), Attempts::Finite(1));
}

#[test]
fn attempts_is_exhausted() {
    let finite = Attempts::Finite(3);
    assert!(!finite.is_exhausted(0));
    assert!(!finite.is_exhausted(1));
    assert!(!finite.is_exhausted(2));
    assert!(finite.is_exhausted(3));
    assert!(finite.is_exhausted(100));

    let forever = Attempts::Forever;
    assert!(!forever.is_exhausted(0));
    assert!(!forever.is_exhausted(1000));
    assert!(!forever.is_exhausted(u32::MAX));
}

#[test]
fn cooldown_parses() {
    let toml = r#"
        on_idle = { action = "nudge", cooldown = "30s" }
    "#;
    #[derive(Debug, Deserialize)]
    struct TestConfig {
        #[serde(default)]
        on_idle: ActionConfig,
    }
    let config: TestConfig = toml::from_str(toml).unwrap();
    assert_eq!(config.on_idle.cooldown(), Some("30s"));
}

#[test]
fn cooldown_simple_action_returns_none() {
    let config = ActionConfig::Simple(AgentAction::Nudge);
    assert_eq!(config.cooldown(), None);
}

#[test]
fn parses_action_with_attempts_and_cooldown() {
    let toml = r#"
        on_idle = { action = "nudge", message = "Keep going", attempts = 5, cooldown = "1m" }
    "#;
    #[derive(Debug, Deserialize)]
    struct TestConfig {
        #[serde(default)]
        on_idle: ActionConfig,
    }
    let config: TestConfig = toml::from_str(toml).unwrap();
    assert_eq!(config.on_idle.action(), &AgentAction::Nudge);
    assert_eq!(config.on_idle.message(), Some("Keep going"));
    assert_eq!(config.on_idle.attempts(), Attempts::Finite(5));
    assert_eq!(config.on_idle.cooldown(), Some("1m"));
}

// =============================================================================
// on_exit alias removal
// =============================================================================

#[test]
fn on_exit_alias_no_longer_populates_on_dead() {
    let toml = r#"
        name = "worker"
        run = "claude"
        on_exit = "done"
    "#;
    // on_exit is now an unknown field; it should be silently ignored
    // and on_dead should retain its default (escalate)
    let agent: AgentDef = toml::from_str(toml).unwrap();
    assert_eq!(
        agent.on_dead.action(),
        &AgentAction::Escalate,
        "on_exit should not populate on_dead"
    );
}
