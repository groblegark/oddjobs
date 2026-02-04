# Workspace Type Refactor

## Overview

Replace the `ephemeral`/`persistent` workspace distinction with two workspace types that describe **what kind of workspace** is created rather than its cleanup policy:

- **`workspace = "folder"`** — Creates an empty directory (current behavior minus worktree cleanup). The engine creates the directory; the runbook decides what to put in it. No git operations performed by the engine.
- **`workspace { git = "worktree" }`** — Creates a git worktree in the workspace directory. The engine handles `git worktree add`, `git worktree remove`, and branch cleanup automatically, eliminating the boilerplate init/abandon/cleanup steps that every runbook currently has.

The `persistent` variant is removed entirely — it was never used in any runbook.

## Project Structure

Key files that need changes (grouped by crate):

```
crates/
├── runbook/src/
│   ├── pipeline.rs          # WorkspaceMode enum → WorkspaceConfig
│   ├── pipeline_tests.rs    # Update tests
│   ├── parser.rs            # Add validation for workspace block
│   └── parser_tests/mod.rs  # Add/update parser tests
├── core/src/
│   ├── effect.rs            # Update CreateWorkspace effect fields
│   ├── event.rs             # Update WorkspaceCreated event fields
│   ├── workspace.rs         # (no change to WorkspaceId/WorkspaceStatus)
│   └── effect_tests.rs      # Update if needed
├── engine/src/
│   ├── runtime/handlers/
│   │   └── pipeline_create.rs  # Generate init/cleanup effects for git worktrees
│   ├── executor.rs             # Handle worktree creation in CreateWorkspace
│   ├── executor_tests.rs       # Update workspace tests
│   ├── workspace.rs            # May add worktree helpers
│   └── runtime/pipeline.rs     # Update cleanup-on-completion logic
├── storage/src/
│   ├── state.rs             # WorkspaceMode → WorkspaceType, remove Persistent
│   └── state_tests/mod.rs   # Update tests
├── daemon/src/
│   └── listener/mutations.rs   # Update workspace drop/prune if needed
└── cli/src/
    └── commands/workspace.rs   # Update display (show type instead of mode)

.oj/runbooks/
├── build.hcl    # Migrate to workspace { git = "worktree" }
├── bug.hcl      # Migrate
├── chore.hcl    # Migrate
├── draft.hcl    # Migrate
├── epic.hcl     # Migrate
├── merge.hcl    # Migrate
└── specs.hcl    # Migrate
```

## Dependencies

No new external dependencies. All required functionality (git commands, serde, HCL parsing) is already available.

## Implementation Phases

### Phase 1: New Workspace Config Type (runbook crate)

Replace the `WorkspaceMode` enum with a richer `WorkspaceConfig` that supports both string and block syntax.

**`crates/runbook/src/pipeline.rs`:**

Replace:
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceMode {
    Ephemeral,
    Persistent,
}
```

With:
```rust
/// Workspace configuration for pipeline execution.
///
/// Supports two forms:
///   `workspace = "folder"`                    — plain directory
///   `workspace { git = "worktree" }`          — git worktree (engine-managed)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WorkspaceConfig {
    /// Short form: `workspace = "folder"`
    Simple(WorkspaceType),
    /// Block form: `workspace { git = "worktree" }`
    Block(WorkspaceBlock),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceType {
    Folder,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceBlock {
    pub git: GitWorkspaceMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GitWorkspaceMode {
    Worktree,
}

impl WorkspaceConfig {
    pub fn is_git_worktree(&self) -> bool {
        matches!(self, WorkspaceConfig::Block(WorkspaceBlock { git: GitWorkspaceMode::Worktree }))
    }
}
```

Update `PipelineDef.workspace` field type from `Option<WorkspaceMode>` to `Option<WorkspaceConfig>`.

Add a parser validation rule: if `workspace` is present but not a recognized form, emit `ParseError::InvalidFormat`.

**Backward compatibility for `"ephemeral"`**: During Phase 1, add custom deserialization that maps `"ephemeral"` → `WorkspaceConfig::Simple(WorkspaceType::Folder)` so existing runbooks and stored WAL events continue to work. Emit a deprecation warning via `ParseError::Warning` (or equivalent) if `"ephemeral"` is encountered during parsing. This mapping can be removed in a future release.

**Milestone:** `cargo test -p oj-runbook` passes with both old `"ephemeral"` and new `"folder"` / `workspace { git = "worktree" }` syntax.

### Phase 2: Update Core Types (core + storage crates)

**`crates/core/src/effect.rs`** — Update `CreateWorkspace`:
```rust
CreateWorkspace {
    workspace_id: WorkspaceId,
    path: PathBuf,
    owner: Option<String>,
    /// "folder" or "worktree"
    workspace_type: Option<String>,
    /// For worktree: the repo root to create the worktree from
    repo_root: Option<PathBuf>,
    /// For worktree: the branch name to create
    branch: Option<String>,
    /// For worktree: the start point (commit/branch to base from)
    start_point: Option<String>,
},
```

**`crates/core/src/event.rs`** — Update `WorkspaceCreated`:
```rust
WorkspaceCreated {
    id: WorkspaceId,
    path: PathBuf,
    branch: Option<String>,
    owner: Option<String>,
    /// "folder" or "worktree" (replaces old "mode" field)
    #[serde(alias = "mode")]
    workspace_type: Option<String>,
},
```

Note the `#[serde(alias = "mode")]` — this ensures old WAL entries with `"mode": "ephemeral"` still deserialize. The `apply_event` code in `state.rs` will map legacy values.

**`crates/storage/src/state.rs`** — Replace `WorkspaceMode`:
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceType {
    #[default]
    Folder,
    Worktree,
}
```

Update `Workspace.mode` → `Workspace.workspace_type` (with `#[serde(alias = "mode")]` for backward compat). In `apply_event`, map legacy mode strings: `"ephemeral"` → `Folder`, `"persistent"` → `Folder`, `"folder"` → `Folder`, `"worktree"` → `Worktree`.

Add a new `DeleteWorkspace` effect variant or field to signal whether worktree cleanup should be engine-managed vs. already handled by the runbook (for the migration period). Simplest approach: the engine always checks if `.git` is a file (current behavior) — this heuristic already handles both cases correctly regardless of workspace type.

**Milestone:** `cargo test -p oj-core -p oj-storage` passes. Old WAL snapshots still deserialize.

### Phase 3: Engine Worktree Lifecycle (engine crate)

This is the core change: the engine now handles worktree init and cleanup for `workspace_type = "worktree"`.

**`crates/engine/src/runtime/handlers/pipeline_create.rs`:**

When `workspace` is `WorkspaceConfig::Block(WorkspaceBlock { git: GitWorkspaceMode::Worktree })`:

1. Compute `repo_root` from `invoke.dir` (run `git rev-parse --show-toplevel`).
2. Compute branch name from `${local.branch}` if set, otherwise generate `ws-<nonce>`.
3. Compute start point: `HEAD` by default (could later support `base` param).
4. Populate `workspace.*` template variables as before (`workspace.id`, `workspace.root`, `workspace.nonce`), plus new `workspace.branch`.
5. Emit `CreateWorkspace` effect with `workspace_type: "worktree"`, `repo_root`, `branch`, and `start_point`.

**`crates/engine/src/executor.rs`** — Update `CreateWorkspace` handler:

For `workspace_type = "worktree"`:
```rust
// 1. Create workspace directory's parent
tokio::fs::create_dir_all(path.parent().unwrap()).await?;

// 2. Run: git -C <repo_root> worktree add -b <branch> <path> <start_point>
let output = tokio::process::Command::new("git")
    .args(["-C", repo_root, "worktree", "add", "-b", &branch, path_str, &start_point])
    .env_remove("GIT_DIR")
    .env_remove("GIT_WORK_TREE")
    .output()
    .await?;
if !output.status.success() {
    // Emit WorkspaceFailed event
}
```

For `workspace_type = "folder"` (or None): keep current behavior — just `create_dir_all`.

**`crates/engine/src/executor.rs`** — `DeleteWorkspace` handler (no change needed):

The existing `.git` file heuristic already handles worktree detection and runs `git worktree remove --force`. This works for both engine-managed and runbook-managed worktrees.

**`crates/engine/src/runtime/pipeline.rs`** — `complete_pipeline`:

Currently checks `ws.mode == Ephemeral` to decide cleanup. Change to: **always clean up workspaces on successful completion** (both `folder` and `worktree` types). This replaces the ephemeral/persistent distinction — all workspaces are cleaned up on success, kept on failure for debugging.

For worktree workspaces, also clean up the branch: after `git worktree remove`, run `git branch -D <branch>` (best-effort). The branch name is stored in the `Workspace.branch` field.

**Milestone:** `cargo test -p oj-engine` passes. A test creates a pipeline with `workspace { git = "worktree" }` and verifies the worktree is created and cleaned up by the engine.

### Phase 4: Migrate Runbooks

Update all `.oj/runbooks/*.hcl` files to use the new syntax and remove manual worktree boilerplate.

**Pattern — before (build.hcl):**
```hcl
pipeline "build" {
  workspace = "ephemeral"
  on_cancel = { step = "abandon" }
  on_fail   = { step = "abandon" }

  locals {
    repo   = "$(git -C ${invoke.dir} rev-parse --show-toplevel)"
    branch = "feature/${var.name}-${workspace.nonce}"
  }

  step "init" {
    run = <<-SHELL
      git -C "${local.repo}" worktree add -b "${local.branch}" "${workspace.root}" HEAD
      mkdir -p plans
    SHELL
    on_done = { step = "plan" }
  }
  # ... work steps ...
  step "abandon" {
    run = <<-SHELL
      git -C "${local.repo}" worktree remove --force "${workspace.root}" 2>/dev/null || true
      git -C "${local.repo}" branch -D "${local.branch}" 2>/dev/null || true
    SHELL
  }
  step "cleanup" {
    run = "git -C \"${local.repo}\" worktree remove ..."
  }
}
```

**Pattern — after:**
```hcl
pipeline "build" {
  workspace {
    git = "worktree"
  }

  locals {
    branch = "feature/${var.name}-${workspace.nonce}"
  }

  step "init" {
    run     = "mkdir -p plans"
    on_done = { step = "plan" }
  }
  # ... work steps (no abandon/cleanup steps for worktree lifecycle) ...
}
```

Key changes per runbook:
- Replace `workspace = "ephemeral"` → `workspace { git = "worktree" }`
- Remove `local.repo` — the engine resolves the repo root
- Keep `local.branch` — the engine uses it if present, otherwise generates one
- Remove init step's `git worktree add` command — engine handles this
- Remove abandon/cleanup steps that only do `git worktree remove` and `git branch -D`
- Remove `on_cancel = { step = "abandon" }` and `on_fail = { step = "abandon" }` if they only existed for worktree cleanup (the engine handles cleanup on cancel/fail too)
- If init step had additional setup beyond `git worktree add` (e.g. `mkdir -p plans`), keep the step with just that setup

**Special cases:**
- **merge.hcl**: Init step does `git fetch` + force-remove stale worktree before `worktree add`. The fetch remains in the init step. The force-remove becomes unnecessary since the engine will manage this. The `worktree add` uses `origin/${var.mr.base}` as start point, not HEAD — need to support a start point config in the workspace block or keep this in the init step.
- **draft-rebase.hcl / draft-refine.hcl**: Init checks out an existing remote branch, not HEAD. Similar start-point question. These pipelines may need `workspace = "folder"` + manual worktree commands until start-point configuration is added, OR add an optional `start_point` field to the workspace block.

Decision: Keep the workspace block simple for now (`git = "worktree"` always uses HEAD). Runbooks that need custom start points (merge, draft-rebase, draft-refine) use `workspace = "folder"` with manual worktree commands. This simplifies the first pass and avoids over-engineering the workspace block. A `ref` or `start_point` field can be added later if needed.

**Milestone:** All runbooks updated. `oj run build test "test"` works end-to-end with engine-managed worktree lifecycle.

### Phase 5: Update CLI and Documentation

**`crates/cli/src/commands/workspace.rs`:**
- Display `type` column instead of `mode` in `oj workspace list`
- Show `folder` or `worktree` instead of `ephemeral`/`persistent`

**`crates/daemon/src/listener/mutations.rs`:**
- No functional changes needed (workspace drop/prune work on workspace records regardless of type)

**Documentation:**
- Update `docs/concepts/RUNBOOKS.md` with new workspace syntax
- Update `docs/GUIDE.md` examples
- Update `CLAUDE.md` architecture section (remove ephemeral/persistent references)

**Milestone:** `oj workspace list` shows correct type. Documentation is current.

### Phase 6: Update Test Fixtures and Final Cleanup

- Update all test fixtures that reference `WorkspaceMode::Ephemeral` or `workspace = "ephemeral"`
- Remove dead code: old `WorkspaceMode` enum from `pipeline.rs` and `state.rs`
- Remove `Persistent` variant entirely
- Remove backward compat `"ephemeral"` mapping from parser (or keep with deprecation warning for one release cycle)
- Run `make ci` to verify everything passes

**Milestone:** `make ci` passes clean. No references to `ephemeral`/`persistent` remain (except WAL backward compat in state.rs).

## Key Implementation Details

### HCL Parsing: String vs Block

The `workspace` field needs to accept both `workspace = "folder"` (string) and `workspace { git = "worktree" }` (block). Serde's `#[serde(untagged)]` enum handles this — the HCL parser already produces either a string or a map for these forms (see how `notify {}` is parsed as a block and `on_done` accepts both string and structured form).

### WAL Backward Compatibility

The WAL stores events as JSON. Old events have `"mode": "ephemeral"`. The migration path:
1. Rename the event field from `mode` to `workspace_type` with `#[serde(alias = "mode")]`
2. In `apply_event`, map `"ephemeral"` and `"persistent"` to `WorkspaceType::Folder`
3. New events use `workspace_type: "folder"` or `workspace_type: "worktree"`

This ensures old snapshots and WAL replays continue to work without migration scripts.

### Branch Name Resolution for Worktrees

For `workspace { git = "worktree" }`, the engine needs a branch name. Resolution order:
1. If `local.branch` is defined in the pipeline locals, use it (already evaluated by the time CreateWorkspace runs)
2. Otherwise, generate `ws-<workspace_nonce>`

This leverages the existing locals evaluation pipeline — `local.branch = "feature/${var.name}-${workspace.nonce}"` is already resolved before the workspace is created.

### Cleanup on All Terminal States

All workspaces (folder and worktree) are cleaned up on successful completion. On failure or cancellation, workspaces are kept for debugging. This matches what `ephemeral` did, but now it's the only behavior — there's no `persistent` option.

For worktree workspaces, cleanup additionally runs `git branch -D <branch>` after `git worktree remove`.

The engine-level cleanup in `complete_pipeline` replaces the per-runbook cleanup steps. The on_cancel/on_fail cleanup is handled by a new `Effect::DeleteWorkspace` emission in `fail_pipeline` and `cancel_pipeline`, matching the `complete_pipeline` pattern.

Wait — currently, on failure, workspaces are **not** deleted (kept for debugging). The `on_cancel`/`on_fail` steps in runbooks explicitly clean up worktrees. With engine-managed worktrees, the engine should handle this too:
- On **success**: delete workspace (worktree remove + branch delete + rmdir)
- On **failure**: keep workspace directory for debugging, but still `git worktree remove` the git linkage (so `git worktree list` stays clean). Or just keep everything as-is.
- On **cancel**: delete workspace (same as success — cancelled work should be cleaned up)

Simplest approach: keep current behavior (only delete on success), let `oj workspace drop` handle manual cleanup. The runbook's `on_fail`/`on_cancel` steps can still include custom cleanup if desired.

### Template Variable: `workspace.branch`

Add a new template variable `workspace.branch` injected alongside `workspace.root`, `workspace.id`, and `workspace.nonce`. This is populated from the resolved branch name (from `local.branch` or auto-generated). Runbooks can reference it in submit steps: `git push origin "${workspace.branch}"`.

## Verification Plan

- [ ] **Unit tests (runbook crate):** Parse `workspace = "folder"`, `workspace { git = "worktree" }`, and legacy `workspace = "ephemeral"` (with deprecation)
- [ ] **Unit tests (storage crate):** WAL events with old `"mode": "ephemeral"` deserialize correctly into `WorkspaceType::Folder`
- [ ] **Unit tests (engine crate):** `CreateWorkspace` with `workspace_type = "worktree"` creates a real git worktree. `DeleteWorkspace` cleans it up.
- [ ] **Integration test:** Create and run a pipeline with `workspace { git = "worktree" }`, verify worktree is created, agent can work in it, and it's cleaned up on completion
- [ ] **Backward compat test:** Old runbooks with `workspace = "ephemeral"` continue to work (parsed as `folder`)
- [ ] **CLI test:** `oj workspace list` shows `folder`/`worktree` type
- [ ] **`make check`** passes (fmt, clippy, build, test, deny)
- [ ] **`make ci`** passes (full CI including cloc and audit)
