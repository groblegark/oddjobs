// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Orphan query handlers.

use std::path::Path;
use std::sync::Arc;

use parking_lot::Mutex;

use oj_engine::breadcrumb::Breadcrumb;

use crate::protocol::{OrphanAgent, OrphanSummary, Response};

/// Handle ListOrphans query by converting breadcrumbs to OrphanSummary.
pub(super) fn handle_list_orphans(orphans: &Arc<Mutex<Vec<Breadcrumb>>>) -> Response {
    let orphans = orphans.lock();
    let summaries = orphans
        .iter()
        .map(|bc| OrphanSummary {
            pipeline_id: bc.pipeline_id.clone(),
            project: bc.project.clone(),
            kind: bc.kind.clone(),
            name: bc.name.clone(),
            current_step: bc.current_step.clone(),
            step_status: bc.step_status.clone(),
            workspace_root: bc.workspace_root.clone(),
            agents: bc
                .agents
                .iter()
                .map(|a| OrphanAgent {
                    agent_id: a.agent_id.clone(),
                    session_name: a.session_name.clone(),
                    log_path: a.log_path.clone(),
                })
                .collect(),
            updated_at: bc.updated_at.clone(),
        })
        .collect();
    Response::Orphans { orphans: summaries }
}

/// Handle DismissOrphan query by removing the orphan from the registry and deleting its breadcrumb.
pub(super) fn handle_dismiss_orphan(
    orphans: &Arc<Mutex<Vec<Breadcrumb>>>,
    id: &str,
    logs_path: &Path,
) -> Response {
    let mut orphans = orphans.lock();

    // Find by exact match or prefix
    let idx = orphans
        .iter()
        .position(|bc| bc.pipeline_id == id || bc.pipeline_id.starts_with(id));

    match idx {
        Some(i) => {
            let removed = orphans.remove(i);
            // Delete the breadcrumb file
            let path = oj_engine::log_paths::breadcrumb_path(logs_path, &removed.pipeline_id);
            let _ = std::fs::remove_file(&path);
            Response::Ok
        }
        None => Response::Error {
            message: format!("orphan not found: {}", id),
        },
    }
}
