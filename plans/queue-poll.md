# Queue Poll

Add a `poll` option to external queue blocks so workers periodically check for new items, enabling use cases where items arrive outside the daemon (cron-populated queues, cross-project pushes).

## Overview

External queues currently only poll when explicitly woken (via `oj queue push`, worker start, or pipeline completion). This plan adds a timer-based periodic poll using the same `SetTimer`/reschedule pattern that cron intervals use. When `poll = "30s"` is set on an external queue, the attached worker sets a recurring timer that triggers `WorkerWake` at the specified interval.

## Project Structure

Files to modify:

```
crates/runbook/src/queue.rs        # Add `poll` field to QueueDef
crates/runbook/src/parser.rs       # Validate poll field (external-only, valid duration)
crates/runbook/src/validate.rs     # Add "ms" suffix support to validate_duration_str
crates/core/src/timer.rs           # Add TimerId::queue_poll constructor
crates/engine/src/monitor.rs       # Add "ms" suffix support to parse_duration
crates/engine/src/runtime/handlers/timer.rs   # Route queue-poll: timers
crates/engine/src/runtime/handlers/worker.rs  # Start/cancel/fire poll timers
```

## Dependencies

No new external dependencies. Uses existing `Effect::SetTimer`/`Effect::CancelTimer` infrastructure and the hand-rolled duration parser.

## Implementation Phases

### Phase 1: Duration parser — add millisecond support

The instructions specify `200ms` as a valid duration, but `parse_duration` and `validate_duration_str` don't support milliseconds.

**`crates/engine/src/monitor.rs` — `parse_duration`:**

Add `"ms"` suffix returning `Duration::from_millis(num)` instead of using the seconds multiplier:

```rust
let multiplier = match suffix.trim() {
    "ms" | "millis" | "millisecond" | "milliseconds" => {
        return Ok(Duration::from_millis(num));
    }
    "" | "s" | "sec" | "secs" | "second" | "seconds" => 1,
    // ... existing suffixes ...
};
```

**`crates/runbook/src/validate.rs` — `validate_duration_str`:**

Add `"ms" | "millis" | "millisecond" | "milliseconds"` to the accepted suffix match arm.

**Verify:** Unit tests for `parse_duration("200ms")` and `validate_duration_str("200ms")`.

### Phase 2: Add `poll` field to QueueDef and validate

**`crates/runbook/src/queue.rs`:**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueDef {
    // ... existing fields ...

    /// Poll interval for external queues (e.g. "30s", "5m")
    /// When set, workers periodically check the queue at this interval
    #[serde(default)]
    pub poll: Option<String>,
}
```

**`crates/runbook/src/parser.rs`** — in the queue validation block (around line 234):

- **External queues:** If `poll` is `Some`, validate it with `validate_duration_str`.
- **Persisted queues:** Reject `poll` field (persisted queues are event-driven via `QueuePushed`).

```rust
QueueType::External => {
    // ... existing list/take/retry validation ...
    if let Some(ref poll) = queue.poll {
        if let Err(e) = validate_duration_str(poll) {
            return Err(ParseError::InvalidFormat {
                location: format!("queue.{}.poll", name),
                message: e,
            });
        }
    }
}
QueueType::Persisted => {
    // ... existing validation ...
    if queue.poll.is_some() {
        return Err(ParseError::InvalidFormat {
            location: format!("queue.{}", name),
            message: "persisted queue must not have 'poll' field".to_string(),
        });
    }
}
```

**Verify:** `cargo test -p oj-runbook` — add parser tests for valid poll values, rejection on persisted queues, and invalid duration strings.

### Phase 3: Add `TimerId::queue_poll` and timer routing

**`crates/core/src/timer.rs`:**

```rust
impl TimerId {
    /// Timer ID for periodic queue polling.
    pub fn queue_poll(worker_name: &str, namespace: &str) -> Self {
        if namespace.is_empty() {
            Self::new(format!("queue-poll:{}", worker_name))
        } else {
            Self::new(format!("queue-poll:{}/{}", namespace, worker_name))
        }
    }

    /// Returns true if this is a queue poll timer.
    pub fn is_queue_poll(&self) -> bool {
        self.0.starts_with("queue-poll:")
    }
}
```

**`crates/engine/src/runtime/handlers/timer.rs`** — add routing before the unknown-timer fallback:

```rust
if let Some(rest) = id_str.strip_prefix("queue-poll:") {
    return self.handle_queue_poll_timer(rest).await;
}
```

**Verify:** `cargo test -p oj-core` for TimerId construction, `cargo check` for compilation.

### Phase 4: Worker lifecycle — start, fire, and cancel poll timers

This is the core phase. Follow the cron pattern: set timer on start, reschedule on fire, cancel on stop.

**`crates/engine/src/runtime/handlers/worker.rs`:**

**4a. Store poll interval in `WorkerState`:**

```rust
pub(crate) struct WorkerState {
    // ... existing fields ...
    /// Poll interval for external queues (None = no periodic polling)
    pub poll_interval: Option<String>,
}
```

**4b. In `handle_worker_started` — set initial poll timer:**

After the initial poll effect (around line 124), if the queue has a `poll` value, set the timer:

```rust
QueueType::External => {
    let list_command = queue_def.list.clone().unwrap_or_default();
    let mut events = self
        .executor
        .execute_all(vec![Effect::PollQueue {
            worker_name: worker_name.to_string(),
            list_command,
            cwd: project_root.to_path_buf(),
        }])
        .await?;

    // Start periodic poll timer if configured
    if let Some(ref poll) = queue_def.poll {
        let duration = crate::monitor::parse_duration(poll).map_err(|e| {
            RuntimeError::InvalidFormat(format!("invalid poll interval '{}': {}", poll, e))
        })?;
        let timer_id = TimerId::queue_poll(worker_name, namespace);
        self.executor
            .execute(Effect::SetTimer {
                id: timer_id,
                duration,
            })
            .await?;
    }

    Ok(events)
}
```

Store the poll interval in `WorkerState`:

```rust
let state = WorkerState {
    // ... existing fields ...
    poll_interval: queue_def.poll.clone(),
};
```

**4c. In `handle_worker_stopped` — cancel poll timer:**

```rust
pub(crate) async fn handle_worker_stopped(
    &self,
    worker_name: &str,
) -> Result<Vec<Event>, RuntimeError> {
    let namespace = {
        let mut workers = self.worker_states.lock();
        if let Some(state) = workers.get_mut(worker_name) {
            state.status = WorkerStatus::Stopped;
            state.namespace.clone()
        } else {
            String::new()
        }
    };

    // Cancel poll timer if it was set
    let timer_id = TimerId::queue_poll(worker_name, &namespace);
    self.executor
        .execute(Effect::CancelTimer { id: timer_id })
        .await?;

    Ok(vec![])
}
```

Note: `CancelTimer` for a non-existent timer is a no-op, so this is safe even when poll is not configured.

**4d. Add `handle_queue_poll_timer` — fire and reschedule:**

```rust
/// Handle a queue poll timer firing: wake the worker and reschedule.
pub(crate) async fn handle_queue_poll_timer(
    &self,
    rest: &str,
) -> Result<Vec<Event>, RuntimeError> {
    // Parse worker name from timer ID (after "queue-poll:" prefix)
    // Format: "worker_name" or "namespace/worker_name"
    let worker_name = rest.rsplit('/').next().unwrap_or(rest);

    let (poll_interval, namespace) = {
        let workers = self.worker_states.lock();
        match workers.get(worker_name) {
            Some(s) if s.status == WorkerStatus::Running => {
                match &s.poll_interval {
                    Some(interval) => (interval.clone(), s.namespace.clone()),
                    None => return Ok(vec![]),
                }
            }
            _ => return Ok(vec![]),
        }
    };

    tracing::debug!(worker = worker_name, "queue poll timer fired");

    // Wake the worker (triggers re-poll of the list command)
    let mut result_events = self.handle_worker_wake(worker_name).await?;

    // Reschedule timer for next interval
    let duration = crate::monitor::parse_duration(&poll_interval).map_err(|e| {
        RuntimeError::InvalidFormat(format!("invalid poll interval '{}': {}", poll_interval, e))
    })?;
    let timer_id = TimerId::queue_poll(worker_name, &namespace);
    self.executor
        .execute(Effect::SetTimer {
            id: timer_id,
            duration,
        })
        .await?;

    Ok(result_events)
}
```

**Verify:** `cargo test --all` passes. Manual test with a runbook containing `poll = "5s"` on an external queue, confirming the worker re-polls at the interval.

### Phase 5: Tests

**Unit tests (in existing test files):**

- `crates/runbook/src/parser_tests.rs`: Parse a queue with `poll = "30s"`, verify `QueueDef.poll` is `Some("30s")`. Parse persisted queue with `poll`, verify error. Parse external queue with `poll = "bogus"`, verify error.
- `crates/core/src/timer_tests.rs`: `TimerId::queue_poll("my-worker", "ns")` produces `"queue-poll:ns/my-worker"`. `is_queue_poll()` returns true.
- `crates/engine/` worker handler tests: Verify `handle_worker_started` emits `SetTimer` when poll is configured. Verify `handle_worker_stopped` emits `CancelTimer`. Verify `handle_queue_poll_timer` calls `handle_worker_wake` and reschedules.

**Integration test (optional):**

A test in `tests/` that starts a worker with a polled external queue, waits for the timer to fire, and verifies items are dispatched.

## Key Implementation Details

**Timer identity:** `queue-poll:{namespace}/{worker_name}` — scoped per worker, not per queue. A single timer per worker is sufficient since each worker sources from exactly one queue.

**Reuse of `handle_worker_wake`:** The poll timer handler simply calls the existing `handle_worker_wake` method, which already handles refreshing the runbook and executing the `list` command. No new poll/dispatch logic is needed.

**No state changes for `WorkerWake`:** The existing `WorkerWake` handler already handles the case where the list command returns no items (no-op). The poll timer will harmlessly re-poll even when empty.

**CancelTimer safety:** Cancelling a timer that doesn't exist is a no-op, so `handle_worker_stopped` unconditionally cancels without checking if poll was configured.

**Runbook hot-reload:** When the runbook is refreshed (which happens on every wake), the poll interval could theoretically change. The current design reschedules with the interval stored in `WorkerState` at start time. To pick up changes, the timer handler could re-read the queue def from the refreshed runbook and update `poll_interval` in `WorkerState`. This is a nice-to-have but not required for the initial implementation.

## Verification Plan

1. `cargo fmt --all -- --check` — formatting
2. `cargo clippy --all-targets --all-features -- -D warnings` — lints
3. `quench check` — project-specific checks
4. `cargo test --all` — all tests pass
5. `cargo build --all` — clean build
6. `cargo audit` — no advisories
7. `cargo deny check licenses bans sources` — license compliance
8. Manual smoke test: create a runbook with an external queue using `poll = "5s"`, start the worker, verify periodic re-polling in logs
