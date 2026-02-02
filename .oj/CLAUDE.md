# Runbooks

Runbooks are written in HCL.

## Available Runbooks

### runbooks/build.hcl
Feature development: init → plan agent → implement agent → submit

```bash
oj run build my-feature "Add user authentication"
```

### runbooks/bugfix.hcl
Bug worker pool: pulls bugs from wok → fix agent → submit → done

```bash
oj run fix "Button doesn't respond to clicks"
oj worker start fix
```

### runbooks/merge.hcl
Local merge queue: merge → check → push (with conflict resolution agent)

```bash
oj queue push merges '{"branch": "fix-123", "title": "fix: button color"}'
oj worker start merge
```

## Writing Runbooks

### Minimal Example

```hcl
command "deploy" {
  args = "<env>"
  run  = { pipeline = "deploy" }
}

pipeline "deploy" {
  vars = ["env"]

  step "build" {
    run     = "make build"
    on_done = { step = "test" }
  }

  step "test" {
    run = "make test"
  }
}

agent "reviewer" {
  run      = "claude --dangerously-skip-permissions"
  on_idle  = { action = "nudge", message = "Keep working." }
  on_dead  = { action = "gate", run = "make check" }
  prompt   = "Review the code."
}
```

## Key Patterns

**Pipeline name templates** — give pipelines human-readable names derived from input:

```hcl
pipeline "build" {
  name = "${var.name}"
  # displays as "auth-a1b2c3d4" instead of "build-a1b2c3d4"
}
```

**Locals** — define variables computed once at pipeline creation, available as `${local.*}`:

```hcl
pipeline "build" {
  vars      = ["name", "instructions"]
  workspace = "ephemeral"

  locals {
    repo   = "$(git -C ${invoke.dir} rev-parse --show-toplevel)"
    branch = "feature/${var.name}-${workspace.nonce}"
    title  = "feat(${var.name}): ${var.instructions}"
  }

  step "init" {
    run = <<-SHELL
      git -C "${local.repo}" worktree add -b "${local.branch}" "${workspace.root}" HEAD
    SHELL
  }
}
```

The `local.repo` pattern resolves the main repository root, letting ephemeral workspace steps push branches and manage worktrees in the original repo.

**Pipeline notifications** — desktop notifications on lifecycle events:

```hcl
pipeline "build" {
  notify {
    on_start = "Building: ${var.name}"
    on_done  = "Build landed: ${var.name}"
    on_fail  = "Build failed: ${var.name}"
  }
}
```

## Best Practices

**Shell scripts:**
- `set -e` is automatic — commands fail on error
- Use newlines, not `&&` chains
- Use `test` command, not `if` statements

**Agents:**
- Always use `run = "claude --dangerously-skip-permissions"`
- Set `on_idle` (nudge/done/fail/escalate/gate) and `on_dead` (done/fail/recover/escalate/gate)
- Keep prompts focused on the task; the orchestrator handles completion

**Steps:**
- Use `on_done = { step = "next" }` for explicit transitions
- Use `on_fail` only for special handling (like conflict resolution)
- Use `run = { agent = "name" }` to invoke agents from pipeline steps

**Workspaces:**
- Use `workspace = "ephemeral"` for isolated git worktrees
- Share build cache via `.cargo/config.toml` pointing at the main repo's target dir

**Workers and queues:**
- Use `queue` + `worker` for pull-based processing
- Queue types: `persisted` (internal) or `external` (backed by external tool)
- Workers have `source`, `handler`, and `concurrency`
