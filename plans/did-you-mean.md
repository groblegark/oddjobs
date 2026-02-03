# Did You Mean? Suggestions for oj CLI

## Overview

Add "did you mean?" suggestions to the oj CLI for two cases:

1. **Typo in resource name**: When a queue/worker/cron name isn't found in the current project's runbooks, fuzzy-match against known names and suggest close matches. E.g. `oj queue items mergeq` → `did you mean: merges?`

2. **Wrong project**: When a resource name isn't found in the current namespace but exists in another project's active state, suggest `--project`. E.g. `oj worker stop fix` from `~/Developer` → `worker 'fix' not found. Did you mean: oj worker stop fix --project oddjobs?`

Applies to all resource-lookup commands: queue (items, drop, push, retry), worker (start, stop, logs), cron (start, stop, once, logs).

## Project Structure

```
crates/
├── runbook/src/
│   └── find.rs              # Add collect_all_workers(), collect_all_crons()
├── daemon/src/listener/
│   ├── suggest.rs           # NEW: edit distance + suggestion helpers
│   ├── queues.rs            # Enhance error messages with suggestions
│   ├── workers.rs           # Enhance error messages, add stop validation
│   ├── crons.rs             # Enhance error messages, add stop validation
│   ├── query.rs             # Enhance queue items / worker logs / cron logs
│   └── mod.rs               # Add `mod suggest;`
└── cli/src/commands/
    ├── queue.rs              # (no changes — suggestions flow via Response::Error)
    ├── worker.rs             # (no changes)
    └── cron.rs               # (no changes)
```

## Dependencies

No new external crates. Edit distance is implemented inline (~15 lines). The `oj_runbook` crate already exposes `collect_all_queues`; this plan adds `collect_all_workers` and `collect_all_crons` following the same pattern.

## Implementation Phases

### Phase 1: Add runbook collection helpers

Add `collect_all_workers()` and `collect_all_crons()` to `crates/runbook/src/find.rs`, mirroring the existing `collect_all_queues()`.

**Files changed:**
- `crates/runbook/src/find.rs` — add two functions
- `crates/runbook/src/lib.rs` — add to `pub use find::{...}`

```rust
// crates/runbook/src/find.rs

/// Scan `.oj/runbooks/` and collect all worker definitions.
pub fn collect_all_workers(runbook_dir: &Path) -> Result<Vec<(String, crate::WorkerDef)>, FindError> {
    // Same pattern as collect_all_queues: scan files, parse, collect workers
}

/// Scan `.oj/runbooks/` and collect all cron definitions.
pub fn collect_all_crons(runbook_dir: &Path) -> Result<Vec<(String, crate::CronDef)>, FindError> {
    // Same pattern as collect_all_queues: scan files, parse, collect crons
}
```

**Verification:** `cargo test -p oj-runbook`

### Phase 2: Add suggestion utility module

Create `crates/daemon/src/listener/suggest.rs` with the core matching logic.

**Files changed:**
- `crates/daemon/src/listener/suggest.rs` — NEW
- `crates/daemon/src/listener/mod.rs` — add `mod suggest;`

The module provides:

```rust
// crates/daemon/src/listener/suggest.rs

/// Levenshtein edit distance between two strings.
pub(super) fn edit_distance(a: &str, b: &str) -> usize {
    // Standard DP implementation, ~15 lines
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut dp = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in 0..=a.len() { dp[i][0] = i; }
    for j in 0..=b.len() { dp[0][j] = j; }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = if a[i-1] == b[j-1] { 0 } else { 1 };
            dp[i][j] = (dp[i-1][j] + 1)
                .min(dp[i][j-1] + 1)
                .min(dp[i-1][j-1] + cost);
        }
    }
    dp[a.len()][b.len()]
}

/// Find similar names from a list of candidates.
/// Returns names within edit distance ≤ max(2, input.len()/3),
/// sorted by distance (closest first). Also includes prefix matches.
pub(super) fn find_similar(input: &str, candidates: &[&str]) -> Vec<String> {
    let threshold = (input.len() / 3).max(2);
    let mut matches: Vec<(usize, String)> = candidates.iter()
        .filter(|c| **c != input)
        .filter_map(|c| {
            let dist = edit_distance(input, c);
            if dist <= threshold || c.starts_with(input) || input.starts_with(c) {
                Some((dist, c.to_string()))
            } else {
                None
            }
        })
        .collect();
    matches.sort_by_key(|(d, _)| *d);
    matches.into_iter().map(|(_, name)| name).collect()
}

/// Format a "did you mean" hint for appending to an error message.
/// Returns empty string if no suggestions.
pub(super) fn format_suggestion(similar: &[String]) -> String {
    match similar.len() {
        0 => String::new(),
        1 => format!("\n\n  did you mean: {}?", similar[0]),
        _ => format!("\n\n  did you mean one of: {}?", similar.join(", ")),
    }
}

/// Check if a resource name exists in another namespace's active state.
/// Returns the namespace name if found.
pub(super) fn find_in_other_namespaces(
    resource_type: ResourceType,
    name: &str,
    current_namespace: &str,
    state: &oj_storage::MaterializedState,
) -> Option<String> {
    match resource_type {
        ResourceType::Queue => {
            state.queue_items.keys()
                .filter_map(|k| {
                    let (ns, qname) = super::query_queues::parse_scoped_key(k);
                    if qname == name && ns != current_namespace { Some(ns) } else { None }
                })
                .next()
        }
        ResourceType::Worker => {
            state.workers.values()
                .find(|w| w.name == name && w.namespace != current_namespace)
                .map(|w| w.namespace.clone())
        }
        ResourceType::Cron => {
            state.crons.values()
                .find(|c| c.name == name && c.namespace != current_namespace)
                .map(|c| c.namespace.clone())
        }
    }
}

pub(super) enum ResourceType { Queue, Worker, Cron }

/// Format a cross-project suggestion.
/// E.g.: "\n\n  did you mean: oj worker stop fix --project oddjobs?"
pub(super) fn format_cross_project_suggestion(
    command_prefix: &str,
    name: &str,
    namespace: &str,
) -> String {
    format!("\n\n  did you mean: {} {} --project {}?", command_prefix, name, namespace)
}
```

**Verification:** Unit tests for `edit_distance` and `find_similar` in `suggest.rs`.

### Phase 3: Enhance mutation handler error messages

Modify the daemon's queue, worker, and cron handlers to include suggestions when resources aren't found.

**Files changed:**
- `crates/daemon/src/listener/queues.rs`
- `crates/daemon/src/listener/workers.rs`
- `crates/daemon/src/listener/crons.rs`

#### 3a. Queue handlers (push, drop, retry)

Each handler calls `load_runbook_for_queue()` which returns `Err(String)` when the queue isn't found. The handler then returns `Response::Error { message }`. Enhance the error message by appending suggestions.

Pattern (applied in `handle_queue_push`, `handle_queue_drop`, `handle_queue_retry`):

```rust
pub(super) fn handle_queue_push(
    project_root: &Path,
    namespace: &str,
    queue_name: &str,
    // ... other args
    state: &Arc<Mutex<MaterializedState>>,
) -> Result<Response, ConnectionError> {
    let runbook = match load_runbook_for_queue(project_root, queue_name) {
        Ok(rb) => rb,
        Err(e) => {
            let hint = suggest_for_queue(project_root, queue_name, namespace, state);
            return Ok(Response::Error { message: format!("{}{}", e, hint) });
        }
    };
    // ... rest unchanged
}
```

Helper function in `queues.rs`:

```rust
fn suggest_for_queue(
    project_root: &Path,
    queue_name: &str,
    namespace: &str,
    state: &Arc<Mutex<MaterializedState>>,
) -> String {
    // 1. Collect all queue names from runbooks
    let runbook_dir = project_root.join(".oj/runbooks");
    let all_queues = oj_runbook::collect_all_queues(&runbook_dir)
        .unwrap_or_default();
    let candidates: Vec<&str> = all_queues.iter().map(|(name, _)| name.as_str()).collect();

    // 2. Check for typo (fuzzy match)
    let similar = suggest::find_similar(queue_name, &candidates);
    if !similar.is_empty() {
        return suggest::format_suggestion(&similar);
    }

    // 3. Check for wrong project (cross-namespace)
    let state = state.lock();
    if let Some(other_ns) = suggest::find_in_other_namespaces(
        suggest::ResourceType::Queue, queue_name, namespace, &state,
    ) {
        return suggest::format_cross_project_suggestion("oj queue push", queue_name, &other_ns);
    }

    String::new()
}
```

Note: `handle_queue_push` already receives `state`; `handle_queue_drop` and `handle_queue_retry` also receive `state`. The existing `load_runbook_for_queue` error path in each handler needs the state parameter passed through. Currently `load_runbook_for_queue` returns before the handler uses state, so state is available.

The `command_prefix` for `format_cross_project_suggestion` varies per command:
- `handle_queue_push` → `"oj queue push"`
- `handle_queue_drop` → `"oj queue drop"`
- `handle_queue_retry` → `"oj queue retry"`

To avoid repeating the suggest logic, extract a shared `suggest_for_queue()` function that takes a `command_prefix` parameter.

#### 3b. Worker handlers (start, stop)

**Worker start** (`handle_worker_start`): Similar to queue handlers — when `load_runbook_for_worker()` fails, collect all worker names from runbooks and check cross-namespace state. This function doesn't currently receive `state`, so add it as a parameter. Update the call site in `listener/mod.rs`.

**Worker stop** (`handle_worker_stop`): Currently has no validation — it just emits `WorkerStopped` regardless. Add existence check:

```rust
pub(super) fn handle_worker_stop(
    worker_name: &str,
    namespace: &str,
    event_bus: &EventBus,
    state: &Arc<Mutex<MaterializedState>>,  // NEW parameter
) -> Result<Response, ConnectionError> {
    // Check if worker exists in state
    let scoped = if namespace.is_empty() {
        worker_name.to_string()
    } else {
        format!("{}/{}", namespace, worker_name)
    };
    let exists = {
        let state = state.lock();
        state.workers.contains_key(&scoped)
    };
    if !exists {
        let hint = suggest_for_worker_from_state(worker_name, namespace, state);
        return Ok(Response::Error {
            message: format!("unknown worker: {}{}", worker_name, hint),
        });
    }

    // ... existing event emission
}
```

For `worker stop`, since there's no `project_root` in the request, runbook scanning isn't available. Suggestions come only from daemon state (other active/stopped workers). This is sufficient because you can only meaningfully stop a worker that has been started (i.e., is in state).

To also support runbook-based suggestions for `worker stop`, add an optional `project_root` to `Request::WorkerStop`:

```rust
Request::WorkerStop {
    worker_name: String,
    #[serde(default)]
    namespace: String,
    #[serde(default)]
    project_root: Option<PathBuf>,  // NEW optional field
}
```

The CLI already has `project_root` available in the command handler — pass it through. The `#[serde(default)]` ensures backward compatibility.

Apply the same pattern to `Request::WorkerWake`.

#### 3c. Cron handlers (start, stop, once)

**Cron start and once**: Same pattern as queue/worker start — when `load_runbook_for_cron()` fails, suggest from runbooks + cross-namespace state. These handlers don't currently receive `state`, so add it as a parameter.

**Cron stop**: Same pattern as worker stop — add validation, suggest from state. Add optional `project_root` to `Request::CronStop` for runbook-based suggestions.

### Phase 4: Enhance query handlers

For query commands that currently return empty results instead of errors, add existence validation and suggestions.

**Files changed:**
- `crates/daemon/src/listener/query.rs`
- `crates/daemon/src/protocol.rs` (add optional `project_root` to query types)
- `crates/cli/src/commands/queue.rs` (pass `project_root` in queries)
- `crates/cli/src/commands/worker.rs` (pass `project_root` in queries)
- `crates/cli/src/commands/cron.rs` (pass `project_root` in queries)

#### 4a. Queue items

Add optional `project_root` to `ListQueueItems`:

```rust
Query::ListQueueItems {
    queue_name: String,
    #[serde(default)]
    namespace: String,
    #[serde(default)]
    project_root: Option<PathBuf>,  // NEW
}
```

In the query handler, when the scoped key doesn't exist in `state.queue_items`:

```rust
Query::ListQueueItems { queue_name, namespace, project_root } => {
    let key = if namespace.is_empty() { queue_name.clone() }
              else { format!("{}/{}", namespace, queue_name) };

    match state.queue_items.get(&key) {
        Some(items) => { /* existing: return QueueItems */ }
        None => {
            // Queue not in state — check if it exists in runbooks
            let in_runbook = project_root.as_ref().map_or(false, |root| {
                oj_runbook::find_runbook_by_queue(&root.join(".oj/runbooks"), &queue_name)
                    .ok().flatten().is_some()
            });
            if in_runbook {
                // Queue exists but has no items
                Response::QueueItems { items: vec![] }
            } else {
                // Queue truly not found — suggest
                let mut candidates: Vec<String> = state.queue_items.keys()
                    .filter_map(|k| {
                        let (ns, name) = query_queues::parse_scoped_key(k);
                        if ns == namespace { Some(name) } else { None }
                    })
                    .collect();
                // Also check runbook definitions
                if let Some(ref root) = project_root {
                    let runbook_queues = oj_runbook::collect_all_queues(
                        &root.join(".oj/runbooks")
                    ).unwrap_or_default();
                    for (name, _) in runbook_queues {
                        if !candidates.contains(&name) {
                            candidates.push(name);
                        }
                    }
                }
                let candidate_refs: Vec<&str> = candidates.iter().map(|s| s.as_str()).collect();
                let similar = suggest::find_similar(&queue_name, &candidate_refs);
                let hint = suggest::format_suggestion(&similar);

                // Cross-namespace check
                let cross = suggest::find_in_other_namespaces(
                    suggest::ResourceType::Queue, &queue_name, &namespace, &state,
                );
                let cross_hint = cross.map(|ns|
                    suggest::format_cross_project_suggestion("oj queue items", &queue_name, &ns)
                ).unwrap_or_default();

                let msg = if hint.is_empty() && cross_hint.is_empty() {
                    format!("unknown queue: {}", queue_name)
                } else if !hint.is_empty() {
                    format!("unknown queue: {}{}", queue_name, hint)
                } else {
                    format!("queue '{}' not found{}", queue_name, cross_hint)
                };
                Response::Error { message: msg }
            }
        }
    }
}
```

Update the CLI to pass `project_root` in the query:
```rust
// crates/cli/src/commands/queue.rs — QueueCommand::Items handler
let request = Request::Query {
    query: Query::ListQueueItems {
        queue_name: queue.clone(),
        namespace: effective_namespace,
        project_root: Some(project_root.to_path_buf()),
    },
};
```

#### 4b. Worker logs

Add optional `project_root` to `GetWorkerLogs`:

```rust
Query::GetWorkerLogs {
    name: String,
    #[serde(default)]
    namespace: String,
    lines: usize,
    #[serde(default)]
    project_root: Option<PathBuf>,  // NEW
}
```

In the query handler, before reading the log file, check if the worker exists in state or runbooks. If the log file doesn't exist AND the worker isn't in state AND isn't in runbooks, return an error with suggestions.

```rust
Query::GetWorkerLogs { name, namespace, lines, project_root } => {
    let scoped = if namespace.is_empty() { name.clone() }
                 else { format!("{}/{}", namespace, name) };
    let log_path = worker_log_path(logs_path, &scoped);

    // If log exists, return it (worker was active at some point)
    if log_path.exists() {
        let content = read_log_file(&log_path, lines);
        return Response::WorkerLogs { log_path, content };
    }

    // Log doesn't exist — check if worker is known
    let in_state = state.workers.contains_key(&scoped);
    let in_runbook = project_root.as_ref().map_or(false, |root| {
        oj_runbook::find_runbook_by_worker(&root.join(".oj/runbooks"), &name)
            .ok().flatten().is_some()
    });

    if in_state || in_runbook {
        // Worker exists but no logs yet
        Response::WorkerLogs { log_path, content: String::new() }
    } else {
        // Worker not found — suggest
        // ... similar to queue items suggestion logic
    }
}
```

#### 4c. Cron logs

Same pattern as worker logs. Add optional `project_root` to `GetCronLogs`, check existence, suggest if not found.

### Phase 5: Tests

**Files changed:**
- `crates/daemon/src/listener/suggest.rs` — unit tests (inline `#[cfg(test)]` module)
- `crates/runbook/src/find_tests.rs` — tests for `collect_all_workers`, `collect_all_crons`
- `crates/daemon/src/listener/queues_tests.rs` — test suggestion in queue error messages
- `crates/daemon/src/listener/workers_tests.rs` — test suggestion in worker error messages
- `crates/daemon/src/listener/crons_tests.rs` — test suggestion in cron error messages (if this file exists; create if needed)

#### Suggest module tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_distance_identical() {
        assert_eq!(edit_distance("foo", "foo"), 0);
    }

    #[test]
    fn edit_distance_one_substitution() {
        assert_eq!(edit_distance("mergeq", "merges"), 1);
    }

    #[test]
    fn edit_distance_insertion() {
        assert_eq!(edit_distance("merg", "merge"), 1);
    }

    #[test]
    fn find_similar_returns_close_matches() {
        let candidates = vec!["merges", "deploys", "builds", "merge-queue"];
        let result = find_similar("mergeq", &candidates);
        assert!(result.contains(&"merges".to_string()));
    }

    #[test]
    fn find_similar_returns_empty_for_no_match() {
        let candidates = vec!["deploys", "builds"];
        let result = find_similar("xyz", &candidates);
        assert!(result.is_empty());
    }

    #[test]
    fn find_similar_includes_prefix_matches() {
        let candidates = vec!["merge-queue", "deploys"];
        let result = find_similar("merge", &candidates);
        assert!(result.contains(&"merge-queue".to_string()));
    }

    #[test]
    fn format_suggestion_single() {
        let similar = vec!["merges".to_string()];
        assert_eq!(format_suggestion(&similar), "\n\n  did you mean: merges?");
    }

    #[test]
    fn format_suggestion_multiple() {
        let similar = vec!["merges".to_string(), "merge-queue".to_string()];
        assert_eq!(
            format_suggestion(&similar),
            "\n\n  did you mean one of: merges, merge-queue?"
        );
    }

    #[test]
    fn format_suggestion_empty() {
        let similar: Vec<String> = vec![];
        assert_eq!(format_suggestion(&similar), "");
    }
}
```

#### Handler integration tests

Test that the full error message includes suggestions by setting up state with known resources and requesting a misspelled one.

## Key Implementation Details

### Edit distance threshold

Use `max(2, input.len() / 3)` as the maximum edit distance. This allows:
- Short names (3-5 chars): up to 2 edits (catches most typos)
- Longer names (6+ chars): scales with length

Also include prefix matches (input is a prefix of a candidate or vice versa) regardless of edit distance, since users often type partial names.

### Suggestion priority

1. **Fuzzy matches from runbooks** (same project) — most likely the user's intent
2. **Cross-namespace matches** (exact name in another project) — second most likely
3. If both exist, prefer the runbook suggestion (typo is more common than wrong project)

### Error message format

Typo case:
```
no runbook found containing queue 'mergeq'

  did you mean: merges?
```

Wrong project case:
```
no runbook found containing worker 'fix'

  did you mean: oj worker stop fix --project oddjobs?
```

### Backward compatibility

- All new protocol fields use `#[serde(default)]` for backward compatibility
- `Request::WorkerStop` and `Request::CronStop` gain optional `project_root` — old messages without it still deserialize
- `Query::ListQueueItems`, `Query::GetWorkerLogs`, `Query::GetCronLogs` gain optional `project_root` — old messages without it still work (suggestions limited to state-only)
- Worker/cron stop now validate existence before emitting events — this is a behavior change (previously silent success for non-existent resources), but is the correct UX

### State access pattern

Several handlers need `state: &Arc<Mutex<MaterializedState>>` added as a parameter:
- `handle_worker_start` — for cross-namespace lookup
- `handle_worker_stop` — for existence check + suggestions
- `handle_cron_start` — for cross-namespace lookup
- `handle_cron_stop` — for existence check + suggestions
- `handle_cron_once` — already has state

Update call sites in `listener/mod.rs` to pass `&state`.

## Verification Plan

1. **Unit tests**: `cargo test -p oj-daemon` — verify edit distance, find_similar, format_suggestion, cross-namespace detection
2. **Runbook tests**: `cargo test -p oj-runbook` — verify collect_all_workers, collect_all_crons
3. **Integration**: `make check` — full build + lint + test suite
4. **Manual testing**: With a running daemon and project:
   - `oj queue items mergeq` when queue "merges" exists → shows suggestion
   - `oj worker stop fix` from wrong directory when worker "fix" exists in another project → shows `--project` suggestion
   - `oj cron start typo` when cron "daily" exists → shows suggestion
   - `oj worker stop validname` when worker exists → works normally (no regression)
   - `oj queue items validqueue` when queue has items → works normally (no regression)
