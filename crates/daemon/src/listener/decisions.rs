// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Decision resolve handler.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;

use oj_core::{Event, PipelineId};
use oj_storage::MaterializedState;

use crate::event_bus::EventBus;
use crate::protocol::Response;

use super::ConnectionError;

pub(super) fn handle_decision_resolve(
    id: &str,
    chosen: Option<usize>,
    message: Option<String>,
    event_bus: &EventBus,
    state: &Arc<Mutex<MaterializedState>>,
) -> Result<Response, ConnectionError> {
    let state_guard = state.lock();

    // Find decision by ID or prefix
    let decision = state_guard
        .get_decision(id)
        .ok_or_else(|| ConnectionError::Internal(format!("decision not found: {}", id)))?;

    // Validate: must be unresolved
    if decision.is_resolved() {
        return Ok(Response::Error {
            message: format!("decision {} is already resolved", id),
        });
    }

    // Validate: choice must be in range if provided
    if let Some(choice) = chosen {
        if choice == 0 || choice > decision.options.len() {
            return Ok(Response::Error {
                message: format!(
                    "choice {} out of range (1..{})",
                    choice,
                    decision.options.len()
                ),
            });
        }
    }

    // Validate: at least one of chosen or message must be provided
    if chosen.is_none() && message.is_none() {
        return Ok(Response::Error {
            message: "must provide either a choice or a message (-m)".to_string(),
        });
    }

    let full_id = decision.id.as_str().to_string();
    let pipeline_id = decision.pipeline_id.clone();
    let decision_namespace = decision.namespace.clone();
    drop(state_guard);

    let resolved_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    // Emit DecisionResolved
    let event = Event::DecisionResolved {
        id: full_id.clone(),
        chosen,
        message: message.clone(),
        resolved_at_ms,
        namespace: decision_namespace,
    };
    event_bus
        .send(event)
        .map_err(|_| ConnectionError::WalError)?;

    // Emit PipelineResume to advance the pipeline out of Waiting
    let resume_message = build_resume_message(chosen, message.as_deref(), &full_id);
    let resume_event = Event::PipelineResume {
        id: PipelineId::new(pipeline_id),
        message: Some(resume_message),
        vars: HashMap::new(),
    };
    event_bus
        .send(resume_event)
        .map_err(|_| ConnectionError::WalError)?;

    Ok(Response::DecisionResolved { id: full_id })
}

/// Build a human-readable resume message from the decision resolution.
fn build_resume_message(chosen: Option<usize>, message: Option<&str>, decision_id: &str) -> String {
    let mut parts = Vec::new();
    if let Some(c) = chosen {
        parts.push(format!("decision {} resolved: option {}", decision_id, c));
    }
    if let Some(m) = message {
        if parts.is_empty() {
            parts.push(format!("decision {} resolved: {}", decision_id, m));
        } else {
            parts.push(format!("message: {}", m));
        }
    }
    parts.join("; ")
}

#[cfg(test)]
#[path = "decisions_tests.rs"]
mod tests;
