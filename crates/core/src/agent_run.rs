// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Standalone agent run entity.
//!
//! An `AgentRun` represents a standalone agent invocation triggered by a
//! `command { run = { agent = "..." } }` block. Unlike job-embedded agents,
//! standalone agents are top-level WAL entities with self-resolving lifecycle.

use crate::action_tracker::ActionTracker;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;

crate::define_id! {
    /// Unique identifier for a standalone agent run.
    pub struct AgentRunId;
}

/// Status of a standalone agent run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRunStatus {
    /// Agent is being spawned
    Starting,
    /// Agent is actively working
    Running,
    /// Waiting for human intervention (escalated)
    Waiting,
    /// Agent completed successfully
    Completed,
    /// Agent failed
    Failed,
    /// Agent escalated to human
    Escalated,
}

impl AgentRunStatus {
    /// Whether this status is terminal (no further transitions expected)
    pub fn is_terminal(&self) -> bool {
        matches!(self, AgentRunStatus::Completed | AgentRunStatus::Failed)
    }
}

impl fmt::Display for AgentRunStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentRunStatus::Starting => write!(f, "starting"),
            AgentRunStatus::Running => write!(f, "running"),
            AgentRunStatus::Waiting => write!(f, "waiting"),
            AgentRunStatus::Completed => write!(f, "completed"),
            AgentRunStatus::Failed => write!(f, "failed"),
            AgentRunStatus::Escalated => write!(f, "escalated"),
        }
    }
}

/// A standalone agent run instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRun {
    pub id: String,
    /// Agent definition name from the runbook
    pub agent_name: String,
    /// Command that triggered this run
    pub command_name: String,
    /// Project namespace
    pub namespace: String,
    /// Directory where the agent runs
    pub cwd: PathBuf,
    /// Runbook content hash for cache lookup
    pub runbook_hash: String,
    /// Current status
    pub status: AgentRunStatus,
    /// UUID of the spawned agent (set on start)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// tmux session ID
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Error message if failed
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Epoch milliseconds when created
    pub created_at_ms: u64,
    /// Epoch milliseconds of last update
    pub updated_at_ms: u64,
    /// Action attempt tracking and agent signal state.
    #[serde(flatten)]
    pub action_tracker: ActionTracker,
    /// Variables passed to the command
    #[serde(default)]
    pub vars: HashMap<String, String>,
    /// Session log file size when idle grace timer was set.
    /// Used to detect activity during the grace period.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_grace_log_size: Option<u64>,
    /// Epoch milliseconds when the last nudge was sent.
    /// Used to suppress auto-resume from our own nudge text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_nudge_at: Option<u64>,
}

impl AgentRun {
    /// Check if the agent run is in a terminal state
    pub fn is_terminal(&self) -> bool {
        self.status.is_terminal()
    }

    /// Increment and return the new attempt count for a given action
    pub fn increment_action_attempt(&mut self, trigger: &str, chain_pos: usize) -> u32 {
        self.action_tracker
            .increment_action_attempt(trigger, chain_pos)
    }

    /// Reset action attempts
    pub fn reset_action_attempts(&mut self) {
        self.action_tracker.reset_action_attempts();
    }

    /// Clear agent signal
    pub fn clear_agent_signal(&mut self) {
        self.action_tracker.clear_agent_signal();
    }
}

/// Builder for `AgentRun` with test defaults.
#[cfg(any(test, feature = "test-support"))]
pub struct AgentRunBuilder {
    id: String,
    agent_name: String,
    command_name: String,
    namespace: String,
    cwd: PathBuf,
    runbook_hash: String,
    status: AgentRunStatus,
    agent_id: Option<String>,
    session_id: Option<String>,
    error: Option<String>,
    created_at_ms: u64,
    updated_at_ms: u64,
    action_tracker: ActionTracker,
    vars: HashMap<String, String>,
    idle_grace_log_size: Option<u64>,
    last_nudge_at: Option<u64>,
}

#[cfg(any(test, feature = "test-support"))]
impl Default for AgentRunBuilder {
    fn default() -> Self {
        Self {
            id: "run-1".to_string(),
            agent_name: "worker".to_string(),
            command_name: "agent_cmd".to_string(),
            namespace: String::new(),
            cwd: PathBuf::from("/tmp/test"),
            runbook_hash: "testhash".to_string(),
            status: AgentRunStatus::Running,
            agent_id: Some("agent-uuid-1".to_string()),
            session_id: Some("sess-1".to_string()),
            error: None,
            created_at_ms: 0,
            updated_at_ms: 0,
            action_tracker: ActionTracker::default(),
            vars: HashMap::new(),
            idle_grace_log_size: None,
            last_nudge_at: None,
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
impl AgentRunBuilder {
    pub fn id(mut self, v: impl Into<String>) -> Self {
        self.id = v.into();
        self
    }
    pub fn agent_name(mut self, v: impl Into<String>) -> Self {
        self.agent_name = v.into();
        self
    }
    pub fn command_name(mut self, v: impl Into<String>) -> Self {
        self.command_name = v.into();
        self
    }
    pub fn namespace(mut self, v: impl Into<String>) -> Self {
        self.namespace = v.into();
        self
    }
    pub fn cwd(mut self, v: impl Into<PathBuf>) -> Self {
        self.cwd = v.into();
        self
    }
    pub fn runbook_hash(mut self, v: impl Into<String>) -> Self {
        self.runbook_hash = v.into();
        self
    }
    pub fn status(mut self, v: AgentRunStatus) -> Self {
        self.status = v;
        self
    }
    pub fn agent_id(mut self, v: impl Into<String>) -> Self {
        self.agent_id = Some(v.into());
        self
    }
    pub fn session_id(mut self, v: impl Into<String>) -> Self {
        self.session_id = Some(v.into());
        self
    }
    pub fn error(mut self, v: impl Into<String>) -> Self {
        self.error = Some(v.into());
        self
    }
    pub fn build(self) -> AgentRun {
        AgentRun {
            id: self.id,
            agent_name: self.agent_name,
            command_name: self.command_name,
            namespace: self.namespace,
            cwd: self.cwd,
            runbook_hash: self.runbook_hash,
            status: self.status,
            agent_id: self.agent_id,
            session_id: self.session_id,
            error: self.error,
            created_at_ms: self.created_at_ms,
            updated_at_ms: self.updated_at_ms,
            action_tracker: self.action_tracker,
            vars: self.vars,
            idle_grace_log_size: self.idle_grace_log_size,
            last_nudge_at: self.last_nudge_at,
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
impl AgentRun {
    /// Create a builder with test defaults.
    pub fn builder() -> AgentRunBuilder {
        AgentRunBuilder::default()
    }
}

#[cfg(test)]
#[path = "agent_run_tests.rs"]
mod tests;
