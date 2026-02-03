# Cron Prune

## Overview

Add an `oj cron prune` CLI command that removes stopped crons from daemon state. This follows the established pattern from `oj worker prune`. The command sends a `CronPrune` request to the daemon, which removes all crons with `status == "stopped"` from materialized state by emitting `CronDeleted` events. Supports `--all` (prune all stopped, no grace period — currently same as default), `--dry-run` (show what would be pruned without acting), and `-o json` output.

## Project Structure

Files to create or modify:

```
crates/core/src/event.rs                 # Add CronDeleted event variant
crates/storage/src/state.rs              # Handle CronDeleted in apply()
crates/engine/src/runtime/handlers/mod.rs # Add CronDeleted to no-op match arm
crates/daemon/src/protocol.rs            # Add Request::CronPrune, Response::CronsPruned
crates/daemon/src/protocol_status.rs     # Add CronEntry struct
crates/daemon/src/listener/mod.rs        # Route CronPrune to handler
crates/daemon/src/listener/mutations.rs  # Add handle_cron_prune()
crates/cli/src/client.rs                 # Add cron_prune() client method
crates/cli/src/commands/cron.rs          # Add Prune subcommand and handler
crates/storage/src/state_tests/mod.rs    # Unit test for CronDeleted event
docs/interface/EVENTS.md                 # Document cron:deleted event
```

## Dependencies

No new external dependencies. All changes use existing crate infrastructure.

## Implementation Phases

### Phase 1: Core Event — `CronDeleted`

Add the `CronDeleted` event variant and its state application logic.

**`crates/core/src/event.rs`** — Add new variant after `CronFired`:

```rust
#[serde(rename = "cron:deleted")]
CronDeleted {
    cron_name: String,
    #[serde(default)]
    namespace: String,
},
```

Update the three match arms in the same file:
- `event_type()` → return `"cron:deleted"`
- `Display` impl → format similarly to `WorkerDeleted` (with namespace handling)
- `pipeline_id()` → falls through to `None`

**`crates/storage/src/state.rs`** — In `apply()`, after the `CronStopped` arm:

```rust
Event::CronDeleted { cron_name, namespace } => {
    let key = scoped_key(namespace, cron_name);
    self.crons.remove(&key);
}
```

**`crates/engine/src/runtime/handlers/mod.rs`** — Add `Event::CronDeleted { .. }` to the no-op match arm alongside `WorkerDeleted`, `PipelineDeleted`, and other state-only events that the engine ignores.

**Verification:** `cargo check --all` passes.

### Phase 2: Protocol Types

**`crates/daemon/src/protocol.rs`** — Add request and response variants.

Request variant (alongside existing cron requests):

```rust
CronPrune {
    all: bool,
    dry_run: bool,
},
```

Response variant (alongside existing prune responses):

```rust
CronsPruned {
    pruned: Vec<CronEntry>,
    skipped: usize,
},
```

**`crates/daemon/src/protocol_status.rs`** — Add entry struct alongside `WorkerEntry`, `AgentEntry`, etc.:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronEntry {
    pub name: String,
    pub namespace: String,
}
```

**Verification:** `cargo check --all` passes.

### Phase 3: Daemon Handler and Routing

**`crates/daemon/src/listener/mutations.rs`** — Add `handle_cron_prune()` following the `handle_worker_prune` pattern exactly:

```rust
/// Removes all stopped crons from state by emitting CronDeleted events.
/// Crons are either "running" or "stopped" — all stopped crons are eligible
/// for pruning with no age threshold.
pub(super) fn handle_cron_prune(
    state: &Arc<Mutex<MaterializedState>>,
    event_bus: &EventBus,
    _all: bool,
    dry_run: bool,
) -> Result<Response, ConnectionError> {
    let mut to_prune = Vec::new();
    let mut skipped = 0usize;

    {
        let state_guard = state.lock();
        for record in state_guard.crons.values() {
            if record.status != "stopped" {
                skipped += 1;
                continue;
            }
            to_prune.push(CronEntry {
                name: record.name.clone(),
                namespace: record.namespace.clone(),
            });
        }
    }

    if !dry_run {
        for entry in &to_prune {
            let event = Event::CronDeleted {
                cron_name: entry.name.clone(),
                namespace: entry.namespace.clone(),
            };
            event_bus
                .send(event)
                .map_err(|_| ConnectionError::WalError)?;
        }
    }

    Ok(Response::CronsPruned {
        pruned: to_prune,
        skipped,
    })
}
```

**`crates/daemon/src/listener/mod.rs`** — Route the request (near the other prune handlers):

```rust
Request::CronPrune { all, dry_run } => {
    mutations::handle_cron_prune(state, event_bus, all, dry_run)
}
```

**Verification:** `cargo check --all` passes.

### Phase 4: CLI Client and Command

**`crates/cli/src/client.rs`** — Add client method:

```rust
pub async fn cron_prune(
    &self,
    all: bool,
    dry_run: bool,
) -> Result<(Vec<oj_daemon::CronEntry>, usize), ClientError> {
    let request = Request::CronPrune { all, dry_run };
    match self.send(&request).await? {
        Response::CronsPruned { pruned, skipped } => Ok((pruned, skipped)),
        Response::Error { message } => Err(ClientError::Rejected(message)),
        _ => Err(ClientError::UnexpectedResponse),
    }
}
```

**`crates/cli/src/commands/cron.rs`** — Add `Prune` variant to `CronCommand`:

```rust
/// Remove stopped crons from daemon state
Prune {
    /// Prune all stopped crons (currently same as default)
    #[arg(long)]
    all: bool,

    /// Show what would be pruned without making changes
    #[arg(long)]
    dry_run: bool,
},
```

Add handler arm in the match block:

```rust
CronCommand::Prune { all, dry_run } => {
    let (pruned, skipped) = client.cron_prune(all, dry_run).await?;

    match format {
        OutputFormat::Text => {
            if dry_run {
                println!("Dry run — no changes made\n");
            }

            for entry in &pruned {
                let label = if dry_run { "Would prune" } else { "Pruned" };
                let ns = if entry.namespace.is_empty() {
                    "(no project)".to_string()
                } else {
                    entry.namespace.clone()
                };
                println!("{} cron '{}' ({})", label, entry.name, ns);
            }

            let verb = if dry_run { "would be pruned" } else { "pruned" };
            println!("\n{} cron(s) {}, {} skipped", pruned.len(), verb, skipped);
        }
        OutputFormat::Json => {
            let obj = serde_json::json!({
                "dry_run": dry_run,
                "pruned": pruned,
                "skipped": skipped,
            });
            println!("{}", serde_json::to_string_pretty(&obj)?);
        }
    }
}
```

**Verification:** `cargo check --all` and `cargo test --all` pass.

### Phase 5: Tests, Documentation, and Final Verification

**`crates/storage/src/state_tests/mod.rs`** — Add unit test:
- Apply `CronStarted` → `CronStopped` → `CronDeleted` events; assert cron is removed from `state.crons`.
- Test that deleting a cron with empty namespace works correctly.

**`docs/interface/EVENTS.md`** — Add `cron:deleted` row to the Cron lifecycle table:

```
| `cron:deleted` | CronDeleted | `cron_name`, `namespace` |
```

Run full `make check` to verify everything passes (fmt, clippy, tests, build, audit, deny).

## Key Implementation Details

- **No age threshold**: Like worker prune, cron prune removes all stopped crons immediately. Crons are explicitly stopped and have no useful state to preserve once stopped. The `--all` flag is accepted for CLI consistency but has no behavioral difference — it exists for forward-compatibility if a grace period is added later.
- **Event-driven state removal**: Following the `WorkerDeleted` pattern, cron removal goes through the event bus so it's captured in the WAL and survives daemon restarts. The `CronDeleted` event is the source of truth — `state.apply()` removes the cron from the HashMap.
- **Scoped keys**: Crons use `scoped_key(namespace, name)` as their HashMap key. The `CronDeleted` event carries both `cron_name` and `namespace` so `apply()` can reconstruct the correct key.
- **No file cleanup needed**: Unlike pipeline/agent prune which delete log files, crons don't have their own log files to clean up.
- **Identical to worker prune pattern**: This is a 1:1 port of worker prune with `Worker` → `Cron` naming. The state structure (`CronStatus::Running`/`CronStatus::Stopped`, stored as `"running"`/`"stopped"` strings in `CronRecord`) and pruning logic (filter on `status != "stopped"`) are identical.

## Verification Plan

1. **Unit test** (`crates/storage/src/state_tests/mod.rs`): Apply `CronStarted` → `CronStopped` → `CronDeleted` events; assert cron is removed from state.
2. **Compile check**: `cargo check --all` — ensures all new types, match arms, and imports are wired correctly.
3. **Full suite**: `make check` — fmt, clippy, tests, build, audit, deny all pass.
4. **Manual smoke test** (optional): Start daemon, start a cron, stop it, run `oj cron prune --dry-run` to see it listed, then `oj cron prune` to remove it, verify `oj cron list` no longer shows it.
