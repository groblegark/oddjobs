# GT / OJ Convergence Analysis

## Executive Summary

Gas Town (GT) is a **154k-line Go CLI/RPC orchestrator** with deep tmux integration, AI-agent lifecycle management, beads-backed state, and a protocol-driven multi-agent architecture (Mayor, Deacon, Witness, Refinery, Polecats).

Oddjobs (OJ) is a **75k-line Rust job engine** with a daemon/WAL persistence model, runbook-defined workflows, agent spawn/monitor/recovery, and adapter-based extensibility.

The two systems have **substantial feature overlap** in five critical areas: tmux session management, agent lifecycle, workspace/git operations, monitoring/health, and work dispatch. They are also **converging intentionally** -- GT's `sling_oj.go` already delegates polecat lifecycle to OJ when `GT_SLING_OJ=1` is set.

This report identifies each area of duplication, recommends ownership, and proposes integration seams.

---

## A. Session / Agent Management

### What GT Does

| Capability | GT File | Lines |
|---|---|---|
| Tmux subprocess wrapper (new-session, kill, send-keys, capture, etc.) | `gastown/internal/tmux/tmux.go` | 1783 |
| Session theming (status bars, colors, icons, keybindings) | `gastown/internal/tmux/theme.go` | ~200 |
| Polecat session lifecycle (start, stop, attach, status) | `gastown/internal/polecat/session_manager.go` | 586 |
| Session identity (name generation, naming conventions) | `gastown/internal/session/identity.go` | ~100 |
| Session startup beacons (predecessor discovery) | `gastown/internal/session/startup.go` | 111 |
| Stale message detection (message-time vs session-creation) | `gastown/internal/session/stale.go` | 47 |
| Zombie session cleanup | `gastown/internal/tmux/tmux.go:1739-1764` | 26 |
| Process tree kill (SIGTERM/SIGKILL with PGID awareness) | `gastown/internal/tmux/tmux.go:255-312` | 57 |
| Agent liveness detection (pane command + child process check) | `gastown/internal/tmux/tmux.go:1254-1321` | 67 |
| Nudge session (literal paste + debounce + Escape + wake + multi-Enter) | `gastown/internal/tmux/tmux.go:862-909` | 47 |
| Bypass permissions warning acceptance | `gastown/internal/tmux/tmux.go:967-997` | 30 |
| Agent state (simple Go enum) | `gastown/internal/agent/state.go` | ~50 |

### What OJ Does

| Capability | OJ File | Lines |
|---|---|---|
| Tmux session adapter (spawn, kill, send, capture, is_alive, exit_code) | `crates/adapters/src/session/tmux.rs` | 307 |
| Session styling (color, title, status bar via configure) | `crates/adapters/src/session/tmux.rs:231-257` | 26 |
| Claude agent adapter (spawn, send, kill, reconnect, get_state) | `crates/adapters/src/agent/claude.rs` | 681 |
| Agent file watcher (session log monitoring + liveness fallback) | `crates/adapters/src/agent/watcher.rs` | ~400 |
| Bypass permissions handling (poll + send "2") | `crates/adapters/src/agent/claude.rs:128-177` | 49 |
| Workspace trust handling (poll + send "1") | `crates/adapters/src/agent/claude.rs:202-251` | 49 |
| Login prompt detection | `crates/adapters/src/agent/claude.rs:267-292` | 25 |
| Agent state machine (Working, WaitingForInput, Failed, Exited, SessionGone) | `crates/core/src/agent.rs` | 80 |
| Monitor state normalization + action effects | `crates/engine/src/monitor.rs` | 522 |
| Spawn effects builder (settings injection, prime scripts, hooks) | `crates/engine/src/spawn.rs` | 414 |
| Agent reconnect (--resume flag support) | `crates/adapters/src/agent/claude.rs:505-553` | 48 |

### Exact Duplications

1. **Tmux new-session / kill-session / has-session / send-keys / capture-pane**: Both systems shell out to `tmux` with nearly identical argument construction. GT has ~1800 lines, OJ has ~300 lines. GT's version is far richer (process tree kill, PGID awareness, nudge serialization, wake-pane SIGWINCH trick).

2. **Bypass permissions acceptance**: GT uses `AcceptBypassPermissionsWarning()` (tmux.go:967) which sends Down+Enter. OJ uses `handle_bypass_permissions_prompt()` (claude.rs:128) which sends "2". Different mechanisms for the same goal.

3. **Agent liveness detection**: GT's `IsAgentAlive()` checks pane command + child processes via `pgrep`. OJ's watcher uses `is_process_running()` (tmux.rs:182) which also checks pane PID + pgrep.

4. **Session styling**: GT's `ConfigureGasTownSession()` applies theme, status bar, keybindings. OJ's `configure()` applies color, title, status. OJ's version is minimal; GT's is exhaustive.

### Recommendation

| Capability | Owner | Rationale |
|---|---|---|
| Tmux wrapper (low-level ops) | **OJ** | OJ's adapter pattern is cleaner. GT should call OJ's session adapter via RPC or shared library. |
| Session theming/keybindings | **GT** | GT-specific UX (cycle bindings, mail click, feed). OJ should expose a `configure()` hook that GT calls. |
| Process tree kill | **GT** | GT's PGID-aware kill with process group enumeration is more robust. OJ's `kill()` is just `tmux kill-session`. This logic should be extracted to a shared library or GT should own shutdown. |
| Agent liveness | **OJ** | OJ's file-watcher approach (session log monitoring) is superior to GT's polling. GT should receive liveness events from OJ rather than checking itself. |
| Bypass/trust/login handling | **OJ** | OJ handles all three prompts in sequence during spawn. GT only handles bypass. OJ's version is more complete. |
| Nudge session | **GT** | GT's nudge is battle-tested with serialization, wake-pane, multi-Enter retry. OJ's `send()` is simpler. GT should expose nudge as a service that OJ calls. |
| --resume / reconnect | **OJ** | OJ has full `AgentReconnectConfig` with session ID tracking. GT has no equivalent. |

---

## B. Work Dispatch

### What GT Does

- `sling.go` (~300+ lines): Unified work dispatch command handling bead resolution, target resolution (rig, polecat, mayor, crew, deacon/dogs), auto-convoy creation, formula instantiation
- `sling_formula.go`: Formula parsing, molecule instantiation, wisp creation
- `sling_batch.go`: Batch slinging (multiple beads to a rig)
- `sling_convoy.go`: Auto-convoy creation for dashboard tracking
- `sling_oj.go` (254 lines): **Bridge code** -- delegates to `oj run gt-sling` when `GT_SLING_OJ=1`
- `sling_target.go`: Target resolution (rig names, polecat names, role names)
- `sling_dog.go`: Dispatch to deacon dogs (kennel workers)

### What OJ Does

- `library/gastown/sling.hcl` (197 lines): Runbook defining sling job with steps: provision (create bead + molecule + worktree), execute (agent), submit (push + MR bead + mail), cleanup (worktree remove)
- `library/gastown/convoy.hcl` (108 lines): Convoy creation and dispatch commands
- `library/gastown/infra.hcl` (73 lines): Queue definitions for merge-requests, bugs, ready-work, witness-inbox, deacon-inbox

### Duplication Analysis

**sling.hcl duplicates a subset of sling.go's logic**:
- Bead creation: Both create task beads via `bd create`
- Molecule instantiation: OJ's provision step creates child beads for each molecule step (load-context, implement, test, submit). GT does this via formula instantiation.
- Worktree creation: Both run `git worktree add -b <branch> <path> origin/<base>`
- MR submission: Both create merge-request beads via `bd create -t merge-request`
- Mail notification: Both create POLECAT_DONE messages via `bd create -t message`

**GT has capabilities OJ lacks**:
- Target resolution (who should do the work)
- Name allocation from a name pool
- Auto-convoy creation for dashboard visibility
- Formula-on-bead (`--on` flag)
- Batch slinging (multiple beads in one command)
- Dog dispatch (deacon helper workers)

### Recommendation

| Capability | Owner | Rationale |
|---|---|---|
| Target resolution | **GT** | This is orchestration-layer logic (who does what). |
| Name allocation | **GT** | GT owns the polecat name pool. OJ should receive the name as input. |
| Bead/molecule lifecycle | **GT** | GT owns the beads model. OJ should receive bead IDs, not create them. |
| Worktree creation | **OJ** | OJ's workspace management is more structured (lifecycle states, cleanup). The sling.hcl `provision` step handles this well. |
| Agent spawn + step execution | **OJ** | This is OJ's core value: job/step state machine with monitoring. |
| MR submission + mail | **Shared** | GT should define the protocol; OJ should execute the steps. Currently duplicated in both sling.hcl and witness handlers. |
| `sling_oj.go` bridge | **Keep** | This is the correct integration pattern. GT resolves target + allocates name, then delegates to OJ for execution. |

---

## C. Merge Queue / Refinery

### What GT Does

- `refinery/engineer.go` (~600 lines): Merge queue processor with priority scoring, claim/release, conflict delegation, test execution, branch management
- `refinery/score.go`: Priority scoring for merge requests
- `refinery/manager.go`: Configuration and lifecycle
- `refinery/types.go`: MergeQueueConfig with strategies (direct_merge, pr_to_main, pr_to_branch, direct_to_branch)

### What OJ Does

- `.oj/runbooks/merge.hcl` (233 lines): Two-queue merge system:
  - `merges` queue (persisted): Fast-path clean merges
  - `merge-conflicts` queue (persisted): Slow-path agent-assisted resolution
  - `merge` job: init -> merge -> push (or queue-conflicts) -> cleanup
  - `merge-conflict` job: init -> merge -> resolve (agent) -> push -> cleanup
  - `conflicts` agent: Claude with prime context + conflict resolution prompt
- `library/gastown/infra.hcl`: External queue backed by beads (`bd list -t merge-request --status open`)

### Duplication Analysis

**Both implement**:
- Queue of merge requests
- Sequential processing (concurrency=1)
- Worktree creation for merge workspace
- Merge attempt -> conflict detection -> resolution path
- Post-merge cleanup (worktree remove, branch delete)
- Push with retry loop

**GT has that OJ lacks**:
- Priority scoring (merge order optimization)
- Multiple merge strategies (direct, PR-to-main, PR-to-branch)
- Integration branch support (per-epic branches)
- Configurable test commands before merge
- Claim/release semantics (prevents double-processing)

**OJ has that GT lacks**:
- Two-queue split (clean merges don't wait for conflict resolution)
- Persisted queue with WAL durability
- Agent-based conflict resolution (Claude resolves conflicts directly)
- Declarative retry semantics (`on_fail = { step = "init", attempts = 3 }`)

### Recommendation

| Capability | Owner | Rationale |
|---|---|---|
| Merge priority scoring | **GT** | Orchestration-level concern. |
| Merge strategy selection | **GT** | Policy decision. |
| Fast-path merge execution | **OJ** | OJ's job/step model handles the mechanics well. |
| Conflict resolution (agent) | **OJ** | OJ's agent spawn + monitor is purpose-built for this. |
| Two-queue architecture | **OJ** | Clean design that GT should adopt (GT processes everything serially today). |
| Retry with backoff | **OJ** | Built into the step state machine. |
| Test execution gate | **Shared** | GT defines when to test; OJ executes the gate step. |

---

## D. Mail / Communication

### What GT Does

- `mail/router.go` (~400 lines): Routes messages to correct beads database based on address (town-level vs rig-level)
- `mail/mailbox.go`: Inbox/outbox operations via `bd list -t message`
- `mail/bd.go`: Low-level beads message operations
- `mail/types.go`: Message types, routing rules
- `mail/resolve.go`: Address resolution (mailing lists, queues, announce channels)
- `notify/`: Notification delivery (desktop notifications, status line)
- `inject/queue.go`: JSONL injection queue for Claude Code hooks (prevents concurrent injection)

### What OJ Does

- `library/gastown/infra.hcl`: External queues backed by `bd list -t message` (witness-inbox, deacon-inbox)
- `library/gastown/sling.hcl:107-118`: Creates messages via `bd create -t message` with from/to labels
- `crates/adapters/src/notify/`: Desktop notification adapter (`notify-send`)
- `crates/engine/src/monitor.rs:255-268`: `Effect::Notify` for desktop notifications

### Duplication Analysis

**Both create beads messages**: GT via `mail.Router.Send()`, OJ via shell `bd create -t message`.

**Both poll for unread messages**: GT via `bd list -t message --label to:X`, OJ via external queue `bd list -t message --label to:witness`.

**Both send desktop notifications**: GT via `notify/`, OJ via `DesktopNotifyAdapter`.

**GT has that OJ lacks**:
- Full routing system (mailing lists, queues, announce channels)
- Address resolution (rig-qualified addresses, town-level addresses)
- Injection queue (for Claude hook-to-agent communication)
- Nudge serialization (prevents interleaving)

**OJ has that GT lacks**:
- Declarative mail polling via queue definitions
- Unified notification pipeline through Effect system

### Recommendation

| Capability | Owner | Rationale |
|---|---|---|
| Mail routing + address resolution | **GT** | GT owns the multi-agent protocol. |
| Message creation (bd create) | **GT** | Protocol messages are GT's domain. |
| Message polling | **OJ** (for OJ-managed agents) | OJ's external queue polling is clean for agents it manages. |
| Desktop notifications | **OJ** | OJ's Effect-based notification is more extensible. GT should emit notification requests to OJ. |
| Injection queue | **GT** | Specific to GT's Claude Code hook architecture. |

---

## E. Monitoring / Health

### What GT Does

- `witness/`: Protocol message handlers (POLECAT_DONE, MERGED, ESCALATED, etc.)
- `deacon/`: Heartbeat monitoring, stuck agent detection, stale hook cleanup
- `doctor/`: 50+ health checks (zombie sessions, orphan processes, config validation, beads integrity, etc.)
- `tmux/tmux.go`: `IsAgentAlive()`, `IsRuntimeRunning()`, `CleanupOrphanedSessions()`
- `session/stale.go`: Stale message detection

### What OJ Does

- `crates/engine/src/monitor.rs`: Agent state monitoring with action dispatch (nudge, resume, escalate, done, fail, gate)
- `crates/adapters/src/agent/watcher.rs`: File-watcher on Claude session logs + periodic liveness polling
- `crates/core/src/agent.rs`: AgentState enum (Working, WaitingForInput, Failed, Exited, SessionGone)
- `crates/engine/src/spawn.rs:17`: LIVENESS_INTERVAL = 30s periodic check
- `crates/daemon/src/lifecycle.rs`: Daemon startup recovery via breadcrumbs (orphan detection)
- Declarative monitoring: `on_idle`, `on_dead`, `on_error`, `on_prompt` in runbook agent definitions

### Duplication Analysis

**Both detect dead/zombie agents**: GT via pane command check + child process enumeration. OJ via session log file watching + tmux liveness polling.

**Both handle crash recovery**: GT via `CleanupOrphanedSessions()` at startup. OJ via breadcrumb-based orphan detection at daemon startup.

**Both implement idle detection**: GT via Witness observation (AI-to-AI). OJ via session log state parsing (WaitingForInput detection).

**Key difference**: GT uses **AI observation** (Witness reads pane output, Deacon checks heartbeats). OJ uses **programmatic monitoring** (file watchers on JSONL session logs, timer-based liveness checks). OJ's approach is more reliable; GT's is more flexible.

### Recommendation

| Capability | Owner | Rationale |
|---|---|---|
| Agent state monitoring (session log) | **OJ** | File-watcher approach is deterministic and fast. |
| Idle/dead/error action dispatch | **OJ** | Declarative `on_idle`/`on_dead`/`on_error` in runbooks is superior to GT's AI-based observation. |
| Protocol message handling (POLECAT_DONE, MERGED) | **GT** | Multi-agent protocol is GT's domain. |
| Comprehensive health checks (doctor) | **GT** | GT's 50+ checks cover the full system. OJ should report its health to GT. |
| Zombie cleanup at startup | **Both** | GT cleans tmux zombies. OJ recovers from breadcrumbs. Both are needed. |

---

## F. Configuration / Context Injection

### What GT Does

- `config/env.go`: `AgentEnv()` -- single source of truth for agent environment variables (GT_ROLE, GT_RIG, BD_ACTOR, GIT_AUTHOR_NAME, etc.)
- `config/`: Runtime config loading, agent config, rig config, account management
- `inject/queue.go`: Queue-based injection for Claude hooks
- `advice/`: Advice system (per-agent contextual guidance)
- `gt prime`: Full context injection at session start (identity, hook, mail, advice, git status)

### What OJ Does

- `crates/engine/src/spawn.rs:257-331`: Environment variable injection (OJ_NAMESPACE, OJ_STATE_DIR, CLAUDE_CONFIG_DIR, CLAUDE_CODE_OAUTH_TOKEN, user env files)
- `crates/engine/src/workspace.rs:42-72`: Prime script generation (writes bash scripts to state dir)
- `crates/engine/src/workspace.rs:93-193`: Settings file injection (Stop hook, SessionStart hooks, Notification hook, PreToolUse hook)
- Runbook `agent.env` blocks: Per-agent environment variables
- Runbook `agent.prime` blocks: Per-agent context injection commands
- Runbook `agent.prompt` blocks: Per-agent system prompts

### Duplication Analysis

**Both inject environment variables**: GT via `tmux.SetEnvironment()`. OJ via `tmux new-session -e KEY=VALUE`.

**Both configure Claude hooks**: GT writes `.claude/settings.local.json` with Stop/SessionStart hooks. OJ writes to state-dir `agents/<id>/claude-settings.json` with Stop/SessionStart/Notification/PreToolUse hooks. OJ's hook injection is more complete.

**Both have "prime" concepts**: GT's `gt prime` runs in the SessionStart hook. OJ's prime writes bash scripts that run via SessionStart hooks.

### Recommendation

| Capability | Owner | Rationale |
|---|---|---|
| Agent identity env vars (GT_ROLE, GT_RIG) | **GT** | GT defines the organizational model. |
| OJ operational env vars (OJ_NAMESPACE, OJ_STATE_DIR) | **OJ** | OJ operational concerns. |
| Claude hook injection | **OJ** | OJ's workspace.rs is more complete (Stop, SessionStart, Notification, PreToolUse). |
| Prime/context injection | **Shared** | GT defines what context to inject; OJ handles the mechanics of when/how to inject it. |
| Advice system | **GT** | GT-specific feature with no OJ equivalent. |

---

## G. Git Operations

### What GT Does

- `git/git.go`: Full git wrapper (clone, fetch, merge, push, worktree add/remove/list, branch operations, sparse checkout)
- `polecat/session_manager.go:117-139`: Clone path resolution for polecat worktrees
- `cmd/sling.go`: Worktree creation during sling dispatch

### What OJ Does

- `library/gastown/sling.hcl:85`: `git worktree add -b <branch> <path> origin/<base>`
- `.oj/runbooks/merge.hcl:67-73`: `git worktree add -b <branch> <path> origin/<base>`
- `.oj/runbooks/merge.hcl:116-121`: `git worktree remove --force <path>`

### Duplication Analysis

**Both create and remove worktrees** for polecat workspaces and merge workspaces. GT does it via Go `exec.Command("git", ...)`. OJ does it via shell steps in runbooks.

**GT has that OJ lacks**: A proper Git abstraction layer (`Git` struct with methods). OJ shells out to git directly.

### Recommendation

| Capability | Owner | Rationale |
|---|---|---|
| Git worktree creation/removal | **OJ** (for OJ-managed jobs) | OJ's runbook steps handle this in context. GT should not manage worktrees for OJ jobs. |
| Git abstraction library | **GT** | GT's `git.Git` wrapper could be exposed for OJ to use, but practically OJ's shell approach works fine for runbooks. |

---

## H. Shutdown / Lifecycle

### What GT Does

- `dog/`: Dog (deacon helper) lifecycle management (add, remove, start, stop, state)
- `dog/session_manager.go`: Dog session lifecycle
- `polecat/session_manager.go:358-382`: Graceful stop (Ctrl-C + KillSessionWithProcesses)
- `tmux/tmux.go:255-506`: Process tree kill with PGID awareness, PID exclusion

### What OJ Does

- Runbook `on_cancel` steps: Declarative cleanup on job cancellation
- `crates/engine/src/steps.rs:55-80`: Failure effects (cancel timers, kill session, emit events)
- `crates/daemon/src/lifecycle.rs`: Daemon shutdown with state persistence (WAL + snapshot)
- Agent `on_stop` config: Signal, Idle, or Escalate behavior

### Duplication Analysis

**Both handle graceful shutdown**: GT with explicit process tree kill. OJ with session kill + on_cancel steps.

**GT's process kill is more thorough**: PGID-aware, descendant enumeration, SIGTERM-then-SIGKILL with grace period. OJ relies on tmux's `kill-session`.

### Recommendation

| Capability | Owner | Rationale |
|---|---|---|
| Process tree kill | **GT** (extract to shared) | GT's implementation handles edge cases OJ doesn't. Should be a shared utility. |
| Job/step cleanup | **OJ** | on_cancel steps are the right abstraction. |
| Daemon lifecycle persistence | **OJ** | WAL/snapshot is OJ's core durability model. |

---

## I. Queue Infrastructure

### What GT Does

- Beads-backed queues: `bd list -t <type> --status open` for polling, `bd update <id> --status in_progress` for claim
- No internal queue persistence -- all state is in beads

### What OJ Does

- **Persisted queues** (`type = "persisted"`): WAL-backed internal queues with push/pop semantics. Used for merge queue.
- **External queues** (`type = "external"`): Shell-command-backed polling (same as GT -- runs `bd list`). Used for beads-backed work.
- Worker definitions: `source` (queue) + `handler` (job) + `concurrency`
- `crates/storage/src/wal.rs`: JSONL WAL with group commit, crash recovery

### Duplication Analysis

OJ's external queues are identical in mechanism to GT's beads polling. OJ adds:
- Persisted queues (WAL-backed, no beads dependency)
- Worker abstraction (concurrency control, automatic dispatch)
- Queue push/pop API (`oj queue push <name> --var key=value`)

GT's beads-backed queue is simpler but less durable for in-flight work.

### Recommendation

| Capability | Owner | Rationale |
|---|---|---|
| Persisted queues | **OJ** | WAL-backed queues are OJ's unique value. GT should not reimplement. |
| External (beads) queues | **OJ** | OJ's external queue abstraction cleanly wraps GT's bd commands. |
| Worker concurrency control | **OJ** | Purpose-built in OJ's engine. |
| Beads as state layer | **GT** | Beads is GT's state primitive. OJ reads from it via external queues. |

---

## J. Runbook vs Formula

### What GT Does

- `.beads/formulas/*.formula.toml`: TOML-based workflow definitions
- Molecule system: Parent bead + child step beads
- Wisp system: Lightweight work items attached to agents
- Formula instantiation: Creates bead hierarchy from template

### What OJ Does

- `.oj/runbooks/*.hcl`: HCL-based workflow definitions with:
  - `command` blocks (CLI entry points)
  - `job` blocks (step sequences with `on_done`/`on_fail`/`on_cancel` transitions)
  - `agent` blocks (AI agent configuration with `on_idle`/`on_dead`/`on_error`/`on_prompt`)
  - `queue` blocks (work sources)
  - `worker` blocks (queue consumers)
  - Variable interpolation (`${var.x}`, `${local.x}`, `${workspace.x}`)
  - Workspace management (`workspace = "folder"`)
- Step state machine: Pending -> Running -> Completed/Failed with transition effects
- Circuit breaker: Max step re-entry count prevents runaway retries

### Comparison

| Aspect | GT Formula | OJ Runbook |
|---|---|---|
| Language | TOML | HCL |
| Step transitions | Implicit (linear sequence) | Explicit (`on_done`, `on_fail` per step) |
| Agent config | External (config files) | Inline (agent blocks with prompt, env, prime) |
| Error handling | Agent-driven (AI decides) | Declarative (`on_error`, `on_dead`, `on_idle` actions) |
| State persistence | Beads (distributed) | WAL + snapshot (local daemon) |
| Variable system | Limited | Rich (vars, locals, workspace, invoke namespaces) |
| Concurrency | Manual (pool of polecats) | Worker concurrency control |
| Resume support | No | Yes (`--resume` with session ID tracking) |

### Recommendation

| Capability | Owner | Rationale |
|---|---|---|
| Workflow definition format | **OJ (HCL runbooks)** | More expressive, better error handling, richer variable system. |
| Beads-backed state (molecules, wisps) | **GT** | GT's organizational state model. |
| Step execution engine | **OJ** | Purpose-built with state machine, timers, retry logic. |
| Formula as high-level template | **GT** | Formulas define "what work to do". Runbooks define "how to execute steps". |

---

## Proposed Responsibility Split

```
                    +---------------------------+
                    |        USER / CLI         |
                    +---------------------------+
                              |
              +---------------+----------------+
              |                                |
     +--------v--------+            +----------v---------+
     |     GT (Go)      |            |     OJ (Rust)      |
     |                  |            |                    |
     | OWNS:            |            | OWNS:              |
     | - Multi-agent    |   RPC /    | - Job execution    |
     |   protocol       |   CLI      |   engine           |
     | - Target         | <-------> | - Agent spawn +    |
     |   resolution     |            |   monitor          |
     | - Name           |            | - Session mgmt     |
     |   allocation     |            |   (tmux adapter)   |
     | - Beads state    |            | - Workspace        |
     |   model          |            |   lifecycle        |
     | - Mail routing   |            | - WAL persistence  |
     | - Priority       |            | - Queue workers    |
     |   scoring        |            | - Runbook engine   |
     | - Health checks  |            | - Resume/reconnect |
     |   (doctor)       |            | - Declarative      |
     | - Advice system  |            |   monitoring       |
     | - Town theming   |            | - Hook injection   |
     | - Convoy mgmt    |            | - Desktop notify   |
     +--------+---------+            +----------+---------+
              |                                |
              +---------------+----------------+
                              |
                    +---------v---------+
                    |   BEADS (state)    |
                    | - Issues, tasks    |
                    | - Messages (mail)  |
                    | - MR beads         |
                    | - Agent beads      |
                    +-------------------+
```

### Integration Seams

1. **GT -> OJ: Job dispatch** (already exists as `sling_oj.go`)
   - GT resolves target, allocates name, creates beads
   - GT calls `oj run <command> --var k=v` to start OJ job
   - OJ manages workspace, agent, monitoring, cleanup

2. **OJ -> GT: Protocol messages** (via beads)
   - OJ creates protocol beads (POLECAT_DONE, MR submission) via `bd create`
   - GT's Witness polls and handles these messages

3. **OJ -> GT: Health reporting** (new seam needed)
   - OJ should expose job/agent health via its daemon RPC
   - GT's doctor should query OJ daemon for OJ-managed agent health

4. **GT -> OJ: Nudge delivery** (new seam needed)
   - GT should call `oj agent send <id> <message>` instead of directly nudging tmux
   - OJ's ClaudeAgentAdapter.send() handles Escape + literal + settle + Enter

5. **Shared: Process tree kill** (extract to shared library)
   - GT's PGID-aware kill logic should be available to OJ
   - OJ's `kill()` on TmuxAdapter should use this enhanced kill

---

## Specific Duplicated Code Paths

| # | GT File:Line | OJ File:Line | What | Severity |
|---|---|---|---|---|
| 1 | `tmux/tmux.go:105-112` | `adapters/session/tmux.rs:54-68` | tmux new-session | Medium |
| 2 | `tmux/tmux.go:228-231` | `adapters/session/tmux.rs:139-151` | tmux kill-session | Medium |
| 3 | `tmux/tmux.go:600-612` | `adapters/session/tmux.rs:153-161` | tmux has-session | Low |
| 4 | `tmux/tmux.go:746-765` | `adapters/session/tmux.rs:97-124` | tmux send-keys (literal + Enter) | **High** |
| 5 | `tmux/tmux.go:1125-1127` | `adapters/session/tmux.rs:163-179` | tmux capture-pane | Low |
| 6 | `tmux/tmux.go:967-997` | `adapters/agent/claude.rs:128-177` | Bypass permissions handling | **High** |
| 7 | `tmux/tmux.go:1254-1321` | `adapters/agent/watcher.rs` (liveness) | Agent alive detection | **High** |
| 8 | `polecat/session_manager.go:152-355` | `adapters/agent/claude.rs:341-503` | Agent spawn (session + env + settings + wait) | **High** |
| 9 | `config/env.go:50-110` | `engine/src/spawn.rs:257-331` | Environment variable injection | Medium |
| 10 | `cmd/sling.go` (worktree add) | `library/gastown/sling.hcl:85` | Git worktree creation | Medium |
| 11 | `refinery/engineer.go` | `.oj/runbooks/merge.hcl` | Merge queue processing | **High** |
| 12 | `mail/router.go` (bd create message) | `library/gastown/sling.hcl:107-118` | Protocol message creation | Medium |
| 13 | `doctor/zombie_check.go` | `daemon/lifecycle.rs` (orphan recovery) | Zombie/orphan detection | Medium |

---

## Priority Actions

### Phase 1: Stop the bleeding (dedup critical paths)

1. **Agent spawn**: When OJ manages a job, GT should NOT also start a tmux session. `sling_oj.go` is the right pattern -- extend it to cover all OJ-managed work.

2. **Monitoring**: For OJ-managed agents, GT should not run its own liveness checks. Instead, GT should query OJ's daemon for agent status via RPC.

3. **Merge queue**: Pick one. OJ's two-queue design (clean + conflicts) is superior. GT's priority scoring should feed into OJ's queue ordering. Migrate GT refinery to dispatch merge jobs to OJ.

### Phase 2: Clean integration seams

4. **Nudge protocol**: Define `oj agent send` as the canonical way to send messages to OJ-managed agents. GT calls this instead of tmux directly.

5. **Process kill**: Extract GT's PGID-aware kill into a shared utility. OJ's TmuxAdapter.kill() should use it.

6. **Environment injection**: GT produces the env map; OJ consumes it. Single source, no duplication.

### Phase 3: Feature migration

7. **Formula -> Runbook**: Migrate GT formulas to OJ runbook format for execution. GT keeps formulas as templates that generate runbook invocations.

8. **External queue consolidation**: OJ's external queue definitions in infra.hcl should be the canonical polling definitions. GT should not independently poll the same beads.

9. **Health reporting**: OJ daemon exposes health endpoint. GT doctor queries it for OJ-managed agent health instead of checking tmux directly.
