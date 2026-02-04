// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Session identifier type for tracking agent sessions.
//!
//! SessionId identifies an agent's underlying session (e.g., a tmux session).
//! This is distinct from AgentId which identifies the logical agent instance.

crate::define_id! {
    /// Unique identifier for an agent session.
    ///
    /// Sessions represent the underlying execution environment for agents,
    /// such as tmux sessions. Multiple agent invocations may share a session,
    /// or each agent may have its own dedicated session.
    pub struct SessionId;
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
