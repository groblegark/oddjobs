# Pipeline Breadcrumb Files for Orphan Detection

## Overview

Add write-only breadcrumb files (`<pipeline-id>.crumb.json`) to `~/.local/state/oj/logs/` that capture pipeline state on creation and each step transition. On daemon startup, scan breadcrumbs and cross-reference with recovered WAL/snapshot state to detect orphaned pipelines — those with a breadcrumb but no matching in-memory pipeline. Surface orphans in `oj status` output with context and investigation commands (`oj pipeline peek`, `oj pipeline attach`). No automatic resume; humans investigate and decide.

## Project Structure

Files to create or modify:

```
crates/
├── engine/src/
│   ├── breadcrumb.rs             # NEW: BreadcrumbWriter — serialize & write .crumb.json
│   ├── log_paths.rs              # Add breadcrumb_path() helper
│   ├── mod.rs                    # Export breadcrumb module
│   └── runtime/
│       ├── mod.rs                # Wire BreadcrumbWriter into Runtime
│       ├── handlers/
│       │   └── pipeline_create.rs  # Write breadcrumb on pipeline creation
│       └── pipeline.rs           # Write breadcrumb on step transitions
├── daemon/src/
│   ├── lifecycle.rs              # Scan breadcrumbs on startup, detect orphans
│   ├── protocol.rs              # Add orphan_count to Status response; add OrphanList query
│   └── listener/
│       └── query.rs             # Handle OrphanList query
└── cli/src/commands/
    └── daemon.rs                # Show orphan count/details in `oj daemon status`
```

## Dependencies

No new external dependencies. Uses existing infrastructure:
- `serde` / `serde_json` for breadcrumb serialization
- `std::fs` for file I/O (same pattern as `PipelineLogger`)
- Existing `log_paths` module for path construction

## Implementation Phases

### Phase 1: Breadcrumb Data Model and Writer

**Goal:** Define the breadcrumb JSON schema and implement the writer that produces `.crumb.json` files.

1. **`crates/engine/src/log_paths.rs`** — Add breadcrumb path helper:
   ```rust
   /// Build the path to a pipeline breadcrumb file.
   ///
   /// Structure: `{logs_dir}/{pipeline_id}.crumb.json`
   pub fn breadcrumb_path(logs_dir: &Path, pipeline_id: &str) -> PathBuf {
       logs_dir.join(format!("{}.crumb.json", pipeline_id))
   }
   ```

2. **`crates/engine/src/breadcrumb.rs`** — New module:
   ```rust
   use serde::Serialize;
   use std::collections::HashMap;
   use std::path::{Path, PathBuf};

   /// Breadcrumb snapshot written to disk on pipeline creation and step transitions.
   /// Write-only during normal operation; read-only during orphan detection at startup.
   #[derive(Debug, Clone, Serialize, Deserialize)]
   pub struct Breadcrumb {
       pub pipeline_id: String,
       pub project: String,           // namespace
       pub kind: String,
       pub name: String,
       pub vars: HashMap<String, String>,
       pub current_step: String,
       pub step_status: String,       // "pending", "running", "waiting", "completed", "failed"
       pub agents: Vec<BreadcrumbAgent>,
       pub workspace_id: Option<String>,
       pub workspace_root: Option<PathBuf>,
       pub updated_at: String,        // ISO 8601 UTC timestamp
   }

   #[derive(Debug, Clone, Serialize, Deserialize)]
   pub struct BreadcrumbAgent {
       pub agent_id: String,
       pub session_name: Option<String>,  // tmux session id
       pub log_path: PathBuf,
   }

   /// Writes breadcrumb files alongside pipeline logs.
   ///
   /// Each write atomically replaces the previous breadcrumb for that pipeline.
   /// Failures are logged via tracing but never propagate — breadcrumbs must not
   /// break the engine.
   pub struct BreadcrumbWriter {
       logs_dir: PathBuf,
   }
   ```

   The writer's `write()` method:
   - Serializes `Breadcrumb` to JSON
   - Writes to a `.tmp` file, then renames to `.crumb.json` (atomic replace)
   - Logs warnings on failure via `tracing::warn!`, never returns errors
   - Follows the same resilience pattern as `PipelineLogger`

   Add a `delete()` method that removes the `.crumb.json` file when a pipeline reaches a terminal state (`done`, `failed`, `cancelled`). This keeps the logs directory clean — only non-terminal pipelines leave breadcrumbs. Terminal pipelines already have their `.log` file and WAL history for auditing.

   Populate `agents` from the pipeline's `step_history` — iterate `step_history` entries that have an `agent_id`, and for each, build a `BreadcrumbAgent` using `log_paths::agent_log_path()` for the log path. The `session_name` comes from `pipeline.session_id` (only the current step's agent will have an active session).

   Populate `updated_at` using the same `format_utc_now()` helper from `pipeline_logger.rs`. Extract that function to a shared utility (or duplicate it — it's 20 lines of pure math with no dependencies).

3. **`crates/engine/src/mod.rs`** — Add `pub mod breadcrumb;` export.

**Verification:** Unit test that `BreadcrumbWriter::write()` produces valid JSON with expected fields, and `delete()` removes the file.

### Phase 2: Write Breadcrumbs on Pipeline Lifecycle Events

**Goal:** Integrate `BreadcrumbWriter` into the engine runtime so breadcrumbs are written on pipeline creation and every step transition.

1. **`crates/engine/src/runtime/mod.rs`** — Add `BreadcrumbWriter` as a field on `Runtime` (or `DaemonRuntime`), initialized alongside `PipelineLogger` with the same `logs_dir`.

2. **`crates/engine/src/runtime/handlers/pipeline_create.rs`** — After the pipeline is created and initial step is set, call `breadcrumb_writer.write(&pipeline)` to write the first breadcrumb.

3. **`crates/engine/src/runtime/pipeline.rs`** — Write breadcrumb after each step transition:
   - In `advance_pipeline()` — after the step change is applied, write breadcrumb with new step
   - In `fail_pipeline()` — write breadcrumb with failure step
   - In `complete_pipeline()` — delete the breadcrumb (pipeline is terminal)
   - In `cancel_pipeline()` (if exists) — delete the breadcrumb

   The write calls should happen after the `Effect::Emit` calls that durably record the state change in the WAL, so breadcrumbs trail the WAL (never ahead of it).

4. **Step status updates** — Also write breadcrumb when step status changes within a step (e.g., `StepStarted` sets status to Running). This captures agent information. Hook into the effect handling for `StepStarted` events since that's when `agent_id` and `session_id` are populated on the pipeline.

**Verification:** Integration test: create a pipeline, advance through steps, verify `.crumb.json` exists with correct `current_step` at each stage, and is deleted when terminal.

### Phase 3: Orphan Detection on Daemon Startup

**Goal:** On daemon startup, scan breadcrumb files and identify orphaned pipelines — those with a breadcrumb but no matching pipeline in recovered state.

1. **`crates/engine/src/breadcrumb.rs`** — Add a standalone scan function:
   ```rust
   /// Scan the logs directory for breadcrumb files and return deserialized breadcrumbs.
   /// Skips files that fail to parse (logs a warning).
   pub fn scan_breadcrumbs(logs_dir: &Path) -> Vec<Breadcrumb> {
       // glob for *.crumb.json, deserialize each
   }
   ```

2. **`crates/daemon/src/lifecycle.rs`** — In `startup_inner()`, after state recovery (WAL replay) and before reconciliation:
   ```rust
   // Detect orphaned pipelines from breadcrumbs
   let breadcrumbs = breadcrumb::scan_breadcrumbs(&logs_dir);
   let orphans: Vec<Breadcrumb> = breadcrumbs
       .into_iter()
       .filter(|b| !state.pipelines.contains_key(&b.pipeline_id))
       .collect();

   if !orphans.is_empty() {
       warn!("{} orphaned pipeline(s) detected from breadcrumbs", orphans.len());
       for orphan in &orphans {
           warn!(
               pipeline_id = %orphan.pipeline_id,
               project = %orphan.project,
               kind = %orphan.kind,
               step = %orphan.current_step,
               "orphaned pipeline detected"
           );
       }
   }
   ```

   Store orphans in the daemon's shared state (e.g., an `Arc<Mutex<Vec<Breadcrumb>>>` field on the daemon runtime or a dedicated `OrphanRegistry`).

3. **Cleanup stale breadcrumbs** — Also during startup, remove breadcrumbs for pipelines that *do* exist in recovered state and are terminal. These are leftover from a crash between terminal state and breadcrumb deletion.

**Verification:** Unit test with fixture breadcrumb files: one matching a recovered pipeline (not orphan), one without a match (orphan), one for a terminal pipeline (cleaned up).

### Phase 4: Surface Orphans in CLI

**Goal:** Show orphaned pipelines in `oj daemon status` and provide investigation commands.

1. **`crates/daemon/src/protocol.rs`** — Extend the `Status` response:
   ```rust
   Response::Status {
       uptime_secs: u64,
       pipelines_active: usize,
       sessions_active: usize,
       orphan_count: usize,  // NEW
   }
   ```

   Add a new query variant for listing orphan details:
   ```rust
   Query::ListOrphans,
   ```

   Add a response type:
   ```rust
   Response::Orphans { orphans: Vec<OrphanSummary> },
   ```

   Where `OrphanSummary` mirrors the breadcrumb fields relevant for display:
   ```rust
   #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
   pub struct OrphanSummary {
       pub pipeline_id: String,
       pub project: String,
       pub kind: String,
       pub name: String,
       pub current_step: String,
       pub step_status: String,
       pub workspace_root: Option<PathBuf>,
       pub agents: Vec<BreadcrumbAgent>,
       pub updated_at: String,
   }
   ```

2. **`crates/daemon/src/listener/query.rs`** — Handle `Query::ListOrphans` by reading from the orphan registry.

3. **`crates/cli/src/commands/daemon.rs`** — In `status()`, after existing output:
   ```
   Status: running
   Version: 0.1.0
   Uptime: 2h 15m
   Pipelines: 3 active
   Sessions: 2 active
   Orphans: 1 detected    ← NEW (only shown when > 0)
   ```

   When orphans exist, print a hint:
   ```
   ⚠ 1 orphaned pipeline detected (missing from WAL/snapshot)
     Run `oj daemon orphans` for details
   ```

4. **`crates/cli/src/commands/daemon.rs`** — Add `oj daemon orphans` subcommand that queries `ListOrphans` and displays:
   ```
   Orphaned Pipelines (not in recovered state):

   ID           PROJECT    KIND      NAME             STEP        STATUS   LAST UPDATED
   a1b2c3d4e5f6 myproject  deploy    deploy-staging   build       running  15m ago

   Investigation commands:
     oj pipeline peek a1b2c3d4     # View last tmux output
     oj pipeline attach a1b2c3d4   # Attach to tmux session
     oj pipeline logs a1b2c3d4     # View pipeline log

   Agent sessions:
     a1b2c3d4-build  session: oj-a1b2c3d4-build  log: ~/.local/state/oj/logs/agent/a1b2c3d4-build.log
   ```

   Note: `peek` and `attach` require the tmux session to still be alive. The breadcrumb stores the session name so the CLI can attempt these. If the session is gone, the commands will report that clearly.

**Verification:** Manual test: create a breadcrumb file without a matching pipeline, restart daemon, run `oj daemon status` and `oj daemon orphans`.

### Phase 5: Breadcrumb Cleanup and Edge Cases

**Goal:** Handle breadcrumb lifecycle edge cases for production robustness.

1. **Terminal state cleanup** — When `reconcile_state()` finds a breadcrumb for a pipeline that exists and is terminal, delete the breadcrumb file. This handles the case where the daemon crashed between marking a pipeline terminal and deleting the breadcrumb.

2. **Pipeline prune cleanup** — When `oj pipeline prune` deletes terminal pipelines from state, also delete any lingering `.crumb.json` files for pruned pipeline IDs. Modify the prune handler to call `breadcrumb_writer.delete(pipeline_id)`.

3. **Orphan dismissal** — Add `oj daemon dismiss-orphan <id>` that deletes the breadcrumb file and removes it from the orphan registry. This is for cases where a human has investigated and determined the orphan is safe to ignore (e.g., the workspace was already cleaned up manually).

4. **Breadcrumb file rotation** — If the daemon finds breadcrumb files older than a configurable threshold (default: 7 days) with no matching pipeline, log a warning and auto-dismiss. Stale breadcrumbs from long-dead pipelines are noise, not signal.

**Verification:** Test prune cleanup, dismiss-orphan command, and stale breadcrumb rotation.

## Key Implementation Details

### Atomic Writes

Breadcrumb writes must be atomic to avoid partial reads during orphan detection:
```rust
fn write(&self, breadcrumb: &Breadcrumb) {
    let path = breadcrumb_path(&self.logs_dir, &breadcrumb.pipeline_id);
    let tmp_path = path.with_extension("crumb.tmp");
    // Write to tmp, then rename
    let json = serde_json::to_string_pretty(breadcrumb).unwrap();
    if let Err(e) = std::fs::write(&tmp_path, json.as_bytes())
        .and_then(|_| std::fs::rename(&tmp_path, &path))
    {
        tracing::warn!(pipeline_id = %breadcrumb.pipeline_id, error = %e, "failed to write breadcrumb");
    }
}
```

### Breadcrumb ≠ Source of Truth

The breadcrumb is a *hint* for orphan detection, not a recovery mechanism. It intentionally does not store enough information to reconstruct pipeline state. The WAL/snapshot is the source of truth. If a breadcrumb exists but the WAL doesn't know about the pipeline, something went wrong — a human must investigate.

### Building the Breadcrumb from Pipeline State

The `BreadcrumbWriter` takes a `&Pipeline` reference and extracts all needed fields. It does not need the full `MaterializedState` — all information is on the `Pipeline` struct:
- `pipeline_id` → `pipeline.id`
- `project` → `pipeline.namespace`
- `kind` → `pipeline.kind`
- `name` → `pipeline.name`
- `vars` → `pipeline.vars`
- `current_step` → `pipeline.step`
- `step_status` → `pipeline.step_status` (serialize the enum)
- `workspace_id` → `pipeline.workspace_id.map(|w| w.to_string())`
- `workspace_root` → `pipeline.workspace_path`
- `agents` → iterate `pipeline.step_history` for entries with `agent_id`, plus current step if it has a `session_id`

### Orphan Registry Lifetime

The orphan registry is populated once at startup and only modified by:
- `dismiss-orphan` command (removes entry)
- Stale breadcrumb rotation (removes old entries)

It is not updated during normal runtime — new pipelines will always have WAL entries, so they'll never appear as orphans.

### Investigation Commands and Orphans

The `peek` and `attach` commands already work by session ID. For orphaned pipelines, the tmux session may or may not still be alive. The breadcrumb stores the session name from when it was last written, allowing the CLI to attempt these commands. The existing error handling in `peek`/`attach` will report clearly if the session is gone.

For orphans where the session is dead, the user can still:
- Check `oj pipeline logs <id>` for the pipeline's activity log
- Check the agent log paths listed in the breadcrumb
- Inspect the workspace directory if it still exists

## Verification Plan

1. **Unit tests** (`crates/engine/src/breadcrumb_tests.rs`):
   - `BreadcrumbWriter::write()` produces valid JSON with all fields
   - `BreadcrumbWriter::delete()` removes the file
   - `scan_breadcrumbs()` correctly parses files and skips corrupt ones
   - Round-trip: write → scan → verify fields match

2. **Integration tests** (step transition breadcrumbs):
   - Create pipeline → verify `.crumb.json` exists with initial step
   - Advance pipeline → verify breadcrumb updated with new step
   - Complete pipeline → verify breadcrumb deleted
   - Fail pipeline → verify breadcrumb deleted

3. **Orphan detection tests**:
   - Place breadcrumb file with no matching pipeline in state → detected as orphan
   - Place breadcrumb file with matching terminal pipeline → cleaned up, not reported
   - Place breadcrumb file with matching active pipeline → not orphaned
   - Corrupt breadcrumb file → skipped with warning, no crash

4. **CLI tests**:
   - `oj daemon status` shows orphan count when > 0, omits line when 0
   - `oj daemon orphans` lists orphan details with investigation commands
   - `oj daemon dismiss-orphan` removes orphan from registry and deletes breadcrumb file

5. **`make check`** — All existing tests continue to pass, clippy clean, no new warnings.
