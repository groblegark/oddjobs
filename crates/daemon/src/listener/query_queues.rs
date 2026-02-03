// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Queue query helpers.

use std::path::Path;

use oj_storage::MaterializedState;

use crate::protocol::{QueueSummary, Response};

/// Build a `Response::Queues` listing all known queues across all namespaces,
/// plus any empty queues defined in the local runbook.
pub(super) fn list_queues(
    state: &MaterializedState,
    project_root: &Path,
    namespace: &str,
) -> Response {
    let mut queues: Vec<QueueSummary> = state
        .queue_items
        .iter()
        .map(|(scoped_key, items)| {
            let (ns, queue_name) = parse_scoped_key(scoped_key);

            let workers: Vec<String> = state
                .workers
                .values()
                .filter(|w| w.queue_name == queue_name && w.namespace == ns)
                .map(|w| w.name.clone())
                .collect();

            QueueSummary {
                name: queue_name,
                namespace: ns,
                queue_type: "persisted".to_string(),
                item_count: items.len(),
                workers,
            }
        })
        .collect();

    // Also include queues defined in the local runbook that may have no items yet
    let runbook_dir = project_root.join(".oj/runbooks");
    let queue_defs = oj_runbook::collect_all_queues(&runbook_dir).unwrap_or_default();
    for (name, def) in queue_defs {
        let already_listed = queues
            .iter()
            .any(|q| q.name == name && q.namespace == namespace);
        if !already_listed {
            let queue_type = match def.queue_type {
                oj_runbook::QueueType::External => "external",
                oj_runbook::QueueType::Persisted => "persisted",
            };
            queues.push(QueueSummary {
                name,
                namespace: namespace.to_string(),
                queue_type: queue_type.to_string(),
                item_count: 0,
                workers: vec![],
            });
        }
    }

    queues.sort_by(|a, b| (&a.namespace, &a.name).cmp(&(&b.namespace, &b.name)));
    Response::Queues { queues }
}

/// Parse a scoped key like "namespace/name" into (namespace, name).
/// Returns ("", key) when no slash is present.
pub(super) fn parse_scoped_key(scoped_key: &str) -> (String, String) {
    if let Some(pos) = scoped_key.find('/') {
        (
            scoped_key[..pos].to_string(),
            scoped_key[pos + 1..].to_string(),
        )
    } else {
        (String::new(), scoped_key.to_string())
    }
}
