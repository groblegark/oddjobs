# Auto-Start Worker on Queue Push

## Overview

When an item is pushed to a persisted queue, automatically start any attached workers that are not already running. Currently `wake_attached_workers()` in `crates/daemon/src/listener/queues.rs` only emits `WorkerWake` for workers with status `"running"`. This change adds auto-start logic: for stopped or never-started workers on persisted queues, emit `RunbookLoaded` + `WorkerStarted` (the same events `handle_worker_start()` produces). After this, remove the now-redundant `oj worker start merge` lines from runbook submit steps.

## Project Structure

Key files:

```
crates/daemon/src/listener/queues.rs    # wake_attached_workers() — main change
crates/daemon/src/listener/workers.rs   # handle_worker_start() — reference for event emission
.oj/runbooks/bugfix.hcl                 # Remove `oj worker start merge` from submit step
.oj/runbooks/build.hcl                  # Remove `oj worker start merge` from submit step
```

## Dependencies

No new external dependencies. Uses existing `sha2`, `serde_json`, `oj_core::Event`, and `oj_runbook` types already imported in the daemon crate.

## Implementation Phases

### Phase 1: Extract runbook hashing into a shared helper

The runbook serialization + SHA256 hashing logic in `handle_worker_start()` (workers.rs:62-82) needs to be reused by `wake_attached_workers()`. Extract it into a shared function within the listener module.

**File:** `crates/daemon/src/listener/workers.rs` (or a shared `mod.rs` / `util.rs` in the listener directory)

```rust
/// Serialize a runbook to JSON and compute its SHA256 hash.
/// Returns (runbook_json, hash_hex).
pub(super) fn hash_runbook(
    runbook: &oj_runbook::Runbook,
) -> Result<(serde_json::Value, String), String> {
    let runbook_json = serde_json::to_value(runbook)
        .map_err(|e| format!("failed to serialize runbook: {}", e))?;
    let canonical = serde_json::to_string(&runbook_json)
        .map_err(|e| format!("failed to serialize runbook: {}", e))?;
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(canonical.as_bytes());
    Ok((runbook_json, format!("{:x}", digest)))
}
```

Then update `handle_worker_start()` to call `hash_runbook()` instead of inlining the logic.

**Milestone:** `make check` passes; `handle_worker_start()` behavior is unchanged.

### Phase 2: Auto-start workers in `wake_attached_workers()`

Modify `wake_attached_workers()` to accept `project_root: &Path` (needed for the `WorkerStarted` event) and add auto-start logic for non-running workers.

**File:** `crates/daemon/src/listener/queues.rs`

**Updated signature:**
```rust
fn wake_attached_workers(
    project_root: &Path,
    queue_name: &str,
    runbook: &oj_runbook::Runbook,
    event_bus: &EventBus,
    state: &Arc<Mutex<MaterializedState>>,
) -> Result<(), ConnectionError>
```

**Updated logic inside the loop over `worker_names`:**

```rust
for name in &worker_names {
    let is_running = {
        let state = state.lock();
        state.workers.get(*name)
            .map(|r| r.status == "running")
            .unwrap_or(false)
    };

    if is_running {
        // Existing behavior: wake the running worker
        let event = Event::WorkerWake {
            worker_name: (*name).to_string(),
        };
        event_bus.send(event).map_err(|_| ConnectionError::WalError)?;
    } else {
        // Auto-start: emit RunbookLoaded + WorkerStarted (same as handle_worker_start)
        let worker_def = runbook.get_worker(name).expect("worker validated above");
        let (runbook_json, runbook_hash) = hash_runbook(runbook)
            .map_err(|msg| ConnectionError::Internal(msg))?;

        event_bus.send(Event::RunbookLoaded {
            hash: runbook_hash.clone(),
            version: 1,
            runbook: runbook_json,
        }).map_err(|_| ConnectionError::WalError)?;

        event_bus.send(Event::WorkerStarted {
            worker_name: (*name).to_string(),
            project_root: project_root.to_path_buf(),
            runbook_hash,
            queue_name: worker_def.source.queue.clone(),
            concurrency: worker_def.concurrency,
        }).map_err(|_| ConnectionError::WalError)?;

        tracing::info!(
            queue = queue_name,
            worker = *name,
            "auto-started worker on queue push"
        );
    }
}
```

Update the call site in `handle_queue_push()` to pass `project_root`:

```rust
wake_attached_workers(project_root, queue_name, &runbook, event_bus, state)?;
```

**Notes:**
- The state lock is scoped narrowly (just to read status) so it doesn't block event emission.
- `WorkerStarted` is idempotent — the runtime's `handle_worker_started` overwrites existing in-memory state, so auto-starting an already-running worker is harmless (acts as a wake).
- This only applies to persisted queues because `handle_queue_push()` already rejects non-persisted queues at line 46, so `wake_attached_workers()` is never called for external queues.

**Milestone:** `make check` passes; pushing to a persisted queue with a stopped worker auto-starts it.

### Phase 3: Remove redundant `oj worker start merge` from runbooks

Remove the `oj worker start merge` line from submit steps in both runbooks since auto-start makes them unnecessary.

**File:** `.oj/runbooks/bugfix.hcl` — line 67

Before:
```hcl
  step "submit" {
    run = <<-SHELL
      git add -A
      git diff --cached --quiet || git commit -m "${local.title}"
      git -C "${local.repo}" push origin "${local.branch}"
      oj queue push merges --var branch="${local.branch}" --var title="${local.title}"
      oj worker start merge
    SHELL
    on_done = { step = "done" }
  }
```

After:
```hcl
  step "submit" {
    run = <<-SHELL
      git add -A
      git diff --cached --quiet || git commit -m "${local.title}"
      git -C "${local.repo}" push origin "${local.branch}"
      oj queue push merges --var branch="${local.branch}" --var title="${local.title}"
    SHELL
    on_done = { step = "done" }
  }
```

**File:** `.oj/runbooks/build.hcl` — line 69

Before:
```hcl
  step "submit" {
    run = <<-SHELL
      git add -A
      git diff --cached --quiet || git commit -m "${local.title}"
      git -C "${local.repo}" push origin "${local.branch}"
      oj queue push merges --var branch="${local.branch}" --var title="${local.title}"
      oj worker start merge
    SHELL
  }
```

After:
```hcl
  step "submit" {
    run = <<-SHELL
      git add -A
      git diff --cached --quiet || git commit -m "${local.title}"
      git -C "${local.repo}" push origin "${local.branch}"
      oj queue push merges --var branch="${local.branch}" --var title="${local.title}"
    SHELL
  }
```

**Milestone:** Runbooks no longer contain manual worker start; `oj queue push merges` alone triggers the merge worker.

## Key Implementation Details

### Why this is safe

1. **Idempotency:** `WorkerStarted` is explicitly documented as idempotent (see workers.rs:17-18). The runtime's `handle_worker_started` overwrites any existing in-memory state, so emitting it for an already-running worker is equivalent to a wake.

2. **Persisted-only scope:** `handle_queue_push()` rejects external queues before reaching `wake_attached_workers()`, so the auto-start path is never hit for external queues. External queues use `oj worker start` explicitly, which remains unchanged.

3. **Event ordering:** `RunbookLoaded` is emitted before `WorkerStarted`, matching the exact ordering in `handle_worker_start()`. The runtime depends on this order to resolve the runbook hash during `handle_worker_started`.

### ConnectionError mapping

The `hash_runbook()` helper returns `Result<_, String>`. Map this to the appropriate `ConnectionError` variant. Check what variants exist — if there's no `Internal` variant, return `Response::Error` instead and restructure accordingly.

### Lock scope

The current code locks `state` for the entire loop. The new code should lock only to read the worker status, then release before emitting events. This avoids holding the lock while doing I/O on the event bus.

## Verification Plan

1. **`make check`** — full CI suite (fmt, clippy, tests, build, audit, deny)
2. **Unit tests:** The existing tests in `crates/daemon/src/listener/workers_tests.rs` validate `handle_worker_start()`. Verify they still pass after extracting `hash_runbook()`.
3. **Manual verification:** Run `oj queue push merges --var branch=test --var title=test` without running `oj worker start merge` first. Confirm the merge worker auto-starts and processes the item.
4. **Idempotency check:** Run `oj worker start merge` then `oj queue push merges ...`. Confirm the already-running worker receives a `WorkerWake` (existing path), not a duplicate `WorkerStarted`.
5. **Runbook validation:** Run `oj run build` and `oj run fix` end-to-end to confirm the submit steps work without the explicit `oj worker start merge` line.
