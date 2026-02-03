# Fix Cron: `oj cron once` Pipeline Creation Bug

## Overview

`oj cron once` silently fails because `handle_cron_once` in the daemon emits a `CommandRun` event, which routes through `handle_command` in the engine. That handler calls `load_runbook_for_command` (which looks up a `command {}` block), but cron runbooks define `cron {}` + `pipeline {}` blocks — no `command {}` block exists, so the lookup fails and the pipeline is never created.

The fix: replace `CommandRun` with a new `CronOnce` event that the engine handles via the same `create_and_start_pipeline` code path used by `handle_cron_timer_fired`. Add comprehensive tests for all cron lifecycle flows.

## Project Structure

Key files to modify:

```
crates/
├── core/src/event.rs                          # Add CronOnce event variant
├── daemon/src/listener/crons.rs               # Fix handle_cron_once to emit CronOnce
├── engine/src/runtime/handlers/mod.rs         # Route CronOnce to new handler
├── engine/src/runtime/handlers/cron.rs        # Add handle_cron_once engine handler
└── engine/src/runtime_tests/
    ├── mod.rs                                 # Add cron module
    └── cron.rs                                # New: cron unit tests
```

## Dependencies

No new external dependencies. All changes use existing crate APIs:
- `oj_core::Event` (add variant)
- `oj_runbook` (existing `find_runbook_by_cron`, `pipeline_display_name`)
- `sha2` (already used in engine cron handler)
- `FakeClock`, `FakeAdapters` (existing test infra)

## Implementation Phases

### Phase 1: Add `CronOnce` Event Variant

**File:** `crates/core/src/event.rs`

Add a new event variant alongside the existing cron events (after `CronFired` at line 283):

```rust
#[serde(rename = "cron:once")]
CronOnce {
    cron_name: String,
    pipeline_id: PipelineId,
    pipeline_name: String,
    project_root: PathBuf,
    runbook_hash: String,
    #[serde(default)]
    namespace: String,
},
```

**Milestone:** `cargo check --all` passes.

### Phase 2: Fix `handle_cron_once` in Daemon

**File:** `crates/daemon/src/listener/crons.rs`

Replace the `CommandRun` event emission (lines 190-198) with the new `CronOnce` event:

```rust
let event = Event::CronOnce {
    cron_name: cron_name.to_string(),
    pipeline_id: pipeline_id.clone(),
    pipeline_name: pipeline_display_name.clone(),
    project_root: project_root.to_path_buf(),
    runbook_hash,
    namespace: namespace.to_string(),
};
```

The rest of the function stays the same — it already validates the cron, loads/hashes the runbook, emits `RunbookLoaded`, and generates a pipeline ID.

**Milestone:** Daemon compiles. `CronOnce` event is emitted instead of `CommandRun`.

### Phase 3: Handle `CronOnce` in Engine

**File:** `crates/engine/src/runtime/handlers/mod.rs`

Add a match arm for `CronOnce` in `handle_event` (near the existing cron event arms, ~line 158):

```rust
Event::CronOnce {
    cron_name,
    pipeline_id,
    pipeline_name,
    project_root,
    runbook_hash,
    namespace,
} => {
    result_events.extend(
        self.handle_cron_once(
            cron_name,
            pipeline_id,
            pipeline_name,
            project_root,
            runbook_hash,
            namespace,
        )
        .await?,
    );
}
```

**File:** `crates/engine/src/runtime/handlers/cron.rs`

Add `handle_cron_once` method. This mirrors the pipeline-creation portion of `handle_cron_timer_fired` (lines 144-182) but without timer rescheduling or runbook refresh (the daemon already loaded and hashed the runbook fresh):

```rust
pub(crate) async fn handle_cron_once(
    &self,
    cron_name: &str,
    pipeline_id: &PipelineId,
    pipeline_name: &str,
    project_root: &Path,
    runbook_hash: &str,
    namespace: &str,
) -> Result<Vec<Event>, RuntimeError> {
    let runbook = self.cached_runbook(runbook_hash)?;

    let mut result_events = Vec::new();

    // Create and start pipeline (same path as handle_cron_timer_fired)
    result_events.extend(
        self.create_and_start_pipeline(CreatePipelineParams {
            pipeline_id: pipeline_id.clone(),
            pipeline_name: pipeline_name.to_string(),
            pipeline_kind: /* extract from runbook cron def */ ,
            vars: HashMap::new(),
            runbook_hash: runbook_hash.to_string(),
            runbook_json: None,
            runbook,
            namespace: namespace.to_string(),
        })
        .await?,
    );

    // Emit CronFired tracking event
    result_events.extend(
        self.executor
            .execute_all(vec![Effect::Emit {
                event: Event::CronFired {
                    cron_name: cron_name.to_string(),
                    pipeline_id: pipeline_id.clone(),
                    namespace: namespace.to_string(),
                },
            }])
            .await?,
    );

    Ok(result_events)
}
```

**Key detail for `pipeline_kind`:** The `CronOnce` event carries `pipeline_name` (the display name like `cleanup/a1b2c3d4`) but `create_and_start_pipeline` needs `pipeline_kind` (the raw pipeline name like `cleanup`). Two options:

1. **Add a `pipeline_kind` field to `CronOnce`** — the daemon already has the raw `pipeline_name` from the cron def before generating the display name. Pass it through.
2. **Extract from the cron def in the runbook** — look up cron → pipeline_name at handler time.

Option 1 is simpler and consistent with how `handle_cron_timer_fired` already has the raw name available. Add `pipeline_kind: String` to the `CronOnce` event and populate it in `handle_cron_once` on the daemon side from the cron def's pipeline reference.

**Milestone:** `cargo check --all` passes. `oj cron once <name>` creates and starts the pipeline.

### Phase 4: Unit Tests for Cron Lifecycle

**File:** `crates/engine/src/runtime_tests/cron.rs` (new)
**File:** `crates/engine/src/runtime_tests/mod.rs` (add `mod cron;`)

Create a test runbook with a cron + pipeline (no `command {}` block, matching real cron usage):

```rust
const CRON_RUNBOOK: &str = r#"
[cron.janitor]
interval = "30m"
run = { pipeline = "cleanup" }

[pipeline.cleanup]

[[pipeline.cleanup.step]]
name = "prune"
run = "echo pruning"
"#;
```

#### Test 1: `cron_once_creates_pipeline`
- Setup with `CRON_RUNBOOK`
- Emit `RunbookLoaded` + `CronOnce` events
- Assert: pipeline exists with correct kind (`cleanup`)
- Assert: pipeline step is `prune`
- Assert: `CronFired` event was emitted

#### Test 2: `cron_start_sets_timer`
- Emit `CronStarted` event
- Assert: cron state is `Running`
- Assert: timer was set (check `FakeClock` or effect output)

#### Test 3: `cron_stop_cancels_timer`
- Start cron, then emit `CronStopped`
- Assert: cron state is `Stopped`
- Assert: timer was cancelled

#### Test 4: `cron_timer_fired_creates_pipeline`
- Start cron, then simulate timer fire via `TimerStart` event
- Assert: pipeline exists and step is running
- Assert: timer rescheduled

#### Test 5: `cron_timer_fired_reloads_runbook`
- Start cron, modify runbook on disk, fire timer
- Assert: runbook hash updated in cron state
- Assert: pipeline uses new runbook content

#### Test 6: `cron_once_pipeline_steps_execute`
- Fire `CronOnce`, then simulate `ShellExited` for the step
- Assert: pipeline completes (reaches terminal state)

**Milestone:** `cargo test -p oj-engine` passes with all cron tests green.

### Phase 5: Integration Tests

**File:** `tests/cron.rs` (new) or extend existing integration test file

Integration tests exercise the full daemon → engine flow:

#### Test 1: `cron_once_end_to_end`
- Write a cron runbook to a temp project
- Send `CronOnce` request through the listener
- Verify pipeline appears in state
- Verify pipeline step starts executing

#### Test 2: `cron_start_and_fire`
- Send `CronStart` request
- Advance `FakeClock` past the interval
- Verify pipeline was created on timer fire

Check the existing integration test patterns in `tests/` to match the project's style. If integration tests require a running daemon that's too heavy to set up, convert these to engine-level tests that exercise the full event chain (RunbookLoaded → CronOnce → pipeline creation → step execution).

**Milestone:** All integration tests pass. `make check` clean.

## Key Implementation Details

### Why a New Event (Not Reusing CommandRun)

The `CommandRun` → `handle_command` path does three things that are wrong for cron:
1. Calls `load_runbook_for_command` → `find_runbook_by_command` (looks for `command {}` blocks)
2. Calls `runbook.get_command(command)` to determine `RunDirective`
3. Uses command-specific variable injection (`invoke.dir`, `args.*`)

Cron runbooks have no `command {}` block. The cron definition already knows which pipeline to run. A dedicated `CronOnce` event avoids misrouting through the command handler entirely.

### Event Flow After Fix

```
CLI: oj cron once janitor
  → Daemon: handle_cron_once
    → Emits: RunbookLoaded { hash, runbook_json }
    → Emits: CronOnce { cron_name, pipeline_id, pipeline_name, pipeline_kind, ... }
  → Engine: handle_event(CronOnce)
    → handle_cron_once
      → cached_runbook(hash)  // populated by RunbookLoaded
      → create_and_start_pipeline(...)
      → Emits: CronFired { cron_name, pipeline_id }
```

This matches the existing `CronStarted` → timer → `handle_cron_timer_fired` → `create_and_start_pipeline` flow.

### Alternative Considered: Emit CronStarted + Immediate TimerStart

Could reuse `CronStarted` with a zero interval or immediately fire the timer. Rejected because:
- Pollutes cron state (would need cleanup after one-shot)
- Timer rescheduling would need special-casing for one-shot
- Semantically different: "once" is not "start a recurring cron"

## Verification Plan

1. **Compile check:** `cargo check --all` — no warnings, no errors
2. **Unit tests:** `cargo test -p oj-engine` — all cron tests pass
3. **Full test suite:** `cargo test --all` — no regressions
4. **Lints:** `cargo clippy --all-targets --all-features -- -D warnings`
5. **Format:** `cargo fmt --all -- --check`
6. **Full verification:** `make check` (runs all of the above plus `cargo audit` and `cargo deny`)
7. **Manual smoke test:** `oj cron once <cron-name>` on a project with a cron runbook — pipeline appears and steps execute
