# Standalone Agents

## Overview

Enable `RunDirective::Agent` on commands so that `oj run <name>` with `command "name" { run = { agent = "..." } }` spawns an agent as a top-level WAL entity — no pipeline wrapper. The agent runs in the invoking directory (no ephemeral worktree), its lifecycle is self-resolving via `on_idle`/`on_dead`/`on_prompt`/`on_error` actions, and it appears alongside pipeline agents in `oj agent list`.

## Project Structure

New and modified files:

```
crates/
├── core/src/
│   ├── agent_run.rs          # NEW: AgentRun struct, AgentRunId, AgentRunStatus
│   ├── event.rs              # MODIFY: add agent_run:* event variants
│   ├── effect.rs             # MODIFY: SpawnAgent gains optional agent_run_id field
│   └── lib.rs                # MODIFY: export agent_run module
├── storage/src/
│   ├── state.rs              # MODIFY: add agent_runs to MaterializedState, apply new events
│   └── state_tests/          # MODIFY: tests for new event materialization
├── engine/src/
│   ├── spawn.rs              # MODIFY: extract pipeline-independent spawn logic
│   ├── runtime/
│   │   ├── monitor.rs        # MODIFY: agent routing handles both pipelines and standalone runs
│   │   ├── agent_run.rs      # NEW: standalone agent lifecycle handling
│   │   └── handlers/
│   │       └── command.rs    # MODIFY: handle RunDirective::Agent
│   └── monitor.rs            # MODIFY: build_action_effects takes trait/enum instead of &Pipeline
├── daemon/src/
│   ├── protocol.rs           # MODIFY: new request/response/query variants
│   ├── lifecycle.rs          # MODIFY: recovery for standalone agents
│   └── listener/             # MODIFY: handle new queries
└── cli/src/commands/
    ├── run.rs                # MODIFY: agent dispatch prints agent run ID (not pipeline ID)
    └── agent.rs              # MODIFY: list/show includes standalone agents
```

## Dependencies

No new external dependencies. All functionality builds on existing crates (`uuid`, `serde`, `tokio`, `parking_lot`).

## Implementation Phases

### Phase 1: Core Types — AgentRun Entity

Add the `AgentRun` state machine and WAL events.

**Files:**

- `crates/core/src/agent_run.rs` (new)
- `crates/core/src/event.rs` (modify)
- `crates/core/src/lib.rs` (modify)
- `crates/storage/src/state.rs` (modify)

**Details:**

1. Create `AgentRunId(String)` newtype (same pattern as `PipelineId`):

```rust
// crates/core/src/agent_run.rs

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AgentRunId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRunStatus {
    Starting,
    Running,
    Waiting,    // escalated, awaiting human
    Completed,
    Failed,
    Escalated,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRun {
    pub id: String,               // AgentRunId value
    pub agent_name: String,       // runbook agent definition name
    pub command_name: String,     // command that triggered this
    pub namespace: String,
    pub cwd: PathBuf,             // invoking directory
    pub runbook_hash: String,     // for looking up agent def
    pub status: AgentRunStatus,
    pub agent_id: Option<String>, // UUID of spawned agent (set on start)
    pub session_id: Option<String>,
    pub error: Option<String>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub action_attempts: HashMap<String, u32>, // same pattern as Pipeline
    pub agent_signal: Option<AgentSignalKind>,
    pub vars: HashMap<String, String>,  // args passed to command
}
```

2. Add WAL event variants to `Event`:

```rust
// in crates/core/src/event.rs

#[serde(rename = "agent_run:created")]
AgentRunCreated {
    id: AgentRunId,
    agent_name: String,
    command_name: String,
    namespace: String,
    cwd: PathBuf,
    runbook_hash: String,
    vars: HashMap<String, String>,
    created_at_epoch_ms: u64,
},

#[serde(rename = "agent_run:started")]
AgentRunStarted {
    id: AgentRunId,
    agent_id: AgentId,
},

#[serde(rename = "agent_run:status_changed")]
AgentRunStatusChanged {
    id: AgentRunId,
    status: AgentRunStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
},

#[serde(rename = "agent_run:deleted")]
AgentRunDeleted { id: AgentRunId },
```

3. Add `agent_runs: HashMap<String, AgentRun>` to `MaterializedState` and implement `apply_event` for the new variants.

**Verification:** `cargo test -p oj-storage` — add unit tests for event materialization (Created → Starting, Started → Running, StatusChanged, Deleted).

### Phase 2: Spawn Refactoring — Decouple from Pipeline

Refactor `build_spawn_effects()` so it doesn't require `&Pipeline`.

**Files:**

- `crates/engine/src/spawn.rs` (modify)
- `crates/engine/src/runtime/monitor.rs` (modify — update call site)
- `crates/engine/src/runtime/pipeline.rs` (modify — update call site)

**Details:**

Introduce a `SpawnContext` struct that abstracts what spawn needs:

```rust
// crates/engine/src/spawn.rs

pub struct SpawnContext<'a> {
    pub owner_id: &'a str,       // pipeline_id or agent_run_id
    pub name: &'a str,           // pipeline name or command name
    pub namespace: &'a str,
    pub vars: &'a HashMap<String, String>,
    pub workspace_path: &'a Path,
}
```

Replace `build_spawn_effects(agent_def, pipeline, pipeline_id, ...)` with `build_spawn_effects(agent_def, ctx, ...)`. The existing pipeline call site constructs `SpawnContext` from `&Pipeline`.

The `Effect::SpawnAgent` variant currently has a `pipeline_id: PipelineId` field. Add an `agent_run_id: Option<AgentRunId>` field (defaulting to `None` for backward compat). For standalone runs, `pipeline_id` can be set to a sentinel or we can change the field to an enum:

```rust
// Option A: Keep pipeline_id, add optional agent_run_id
SpawnAgent {
    agent_id: AgentId,
    agent_name: String,
    pipeline_id: PipelineId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agent_run_id: Option<AgentRunId>,
    // ... rest unchanged
}
```

For standalone agents, `pipeline_id` is set to a synthetic value like `PipelineId::new("")` (empty) and `agent_run_id` is `Some(...)`. The agent adapter already only uses `agent_id` for session naming, so `pipeline_id` is effectively a routing key — the new `agent_run_id` takes precedence when present.

**Verification:** `cargo test -p oj-engine` — existing spawn tests still pass, no behavioral change yet.

### Phase 3: Engine — Standalone Agent Lifecycle

Add the runtime handler for standalone agent runs with self-resolving lifecycle.

**Files:**

- `crates/engine/src/runtime/agent_run.rs` (new)
- `crates/engine/src/runtime/monitor.rs` (modify — agent routing)
- `crates/engine/src/runtime/mod.rs` (modify — export)
- `crates/engine/src/monitor.rs` (modify — generalize `build_action_effects`)

**Details:**

1. **Agent routing**: The existing `agent_pipelines: Arc<Mutex<HashMap<AgentId, String>>>` maps agent → pipeline. Add a parallel map `agent_runs: Arc<Mutex<HashMap<AgentId, AgentRunId>>>` to `Runtime`. When an `AgentStateChanged` event arrives, check `agent_runs` first; if found, route to standalone handler. Otherwise fall through to pipeline handler.

2. **Standalone lifecycle handler** (`runtime/agent_run.rs`):

```rust
impl Runtime<S, A, N, C> {
    /// Spawn a standalone agent for a command run
    pub(crate) async fn spawn_standalone_agent(
        &self,
        agent_run_id: &AgentRunId,
        agent_def: &AgentDef,
        agent_name: &str,
        input: &HashMap<String, String>,
        cwd: &Path,
        namespace: &str,
        runbook_hash: &str,
    ) -> Result<Vec<Event>, RuntimeError> { ... }

    /// Handle lifecycle state change for a standalone agent
    pub(crate) async fn handle_standalone_monitor_state(
        &self,
        agent_run: &AgentRun,
        agent_def: &AgentDef,
        state: MonitorState,
    ) -> Result<Vec<Event>, RuntimeError> { ... }

    /// Handle agent:signal for a standalone agent
    pub(crate) async fn handle_standalone_agent_done(
        &self,
        agent_id: &AgentId,
        kind: AgentSignalKind,
        message: Option<String>,
    ) -> Result<Vec<Event>, RuntimeError> { ... }
}
```

3. **Self-resolving actions**: Instead of advancing/failing a pipeline, actions emit `AgentRunStatusChanged` events:
   - `done` → `AgentRunStatusChanged { status: Completed }`
   - `fail` → `AgentRunStatusChanged { status: Failed, reason }`
   - `escalate` → `AgentRunStatusChanged { status: Escalated, reason }`
   - `gate` → runs command; exit 0 → Completed, non-zero → Escalated
   - `nudge` → sends message to agent (same as pipeline)
   - `recover` → kills session, respawns agent (same as pipeline)

4. **Generalize `build_action_effects`**: The current function takes `&Pipeline` to build `StepWaiting`/`StepCompleted` events. For standalone agents, create a parallel function or use an enum parameter:

```rust
pub enum ActionTarget<'a> {
    Pipeline(&'a Pipeline),
    AgentRun(&'a AgentRun),
}

pub fn build_action_effects(
    target: ActionTarget<'_>,
    agent_def: &AgentDef,
    action_config: &ActionConfig,
    trigger: &str,
    input: &HashMap<String, String>,
) -> Result<ActionEffects, RuntimeError> { ... }
```

The `ActionEffects` enum gains variants for standalone outcomes:

```rust
pub enum ActionEffects {
    // existing...
    Nudge { effects: Vec<Effect> },
    AdvancePipeline,
    FailPipeline { error: String },
    Recover { ... },
    Escalate { effects: Vec<Effect> },
    Gate { command: String },
    // new for standalone:
    CompleteAgentRun,
    FailAgentRun { error: String },
    EscalateAgentRun { effects: Vec<Effect> },
}
```

**Verification:** `cargo test -p oj-engine` — unit tests for standalone lifecycle transitions.

### Phase 4: Command Handler + Daemon IPC

Wire up the `RunDirective::Agent` in the command handler and add daemon-side IPC.

**Files:**

- `crates/engine/src/runtime/handlers/command.rs` (modify)
- `crates/daemon/src/protocol.rs` (modify)
- `crates/daemon/src/listener/` (modify)
- `crates/daemon/src/lifecycle.rs` (modify — recovery)

**Details:**

1. **Command handler** (`handlers/command.rs`): Replace the `Err(invalid_directive(...))` for `RunDirective::Agent` with:
   - Load the agent definition from the runbook
   - Generate `AgentRunId` (UUID)
   - Emit `AgentRunCreated` event
   - Call `spawn_standalone_agent()`
   - Emit `AgentRunStarted` event
   - Return a new `Response::AgentRunStarted { agent_run_id, agent_name }`

2. **Protocol additions**:

```rust
// Response
AgentRunStarted {
    agent_run_id: String,
    agent_name: String,
},

// Query extensions
// ListAgents already works — extend the handler to also scan agent_runs
// GetAgent already works — extend to also search agent_runs
// GetAgentLogs — extend to support standalone agent logs
```

3. **Agent list/show handlers**: When building `AgentSummary` for standalone agents, set `pipeline_id: ""` and `step_name: ""` (or `"—"`). The `agent_id`, `agent_name`, `status`, and counters come from `AgentRun` + agent file watcher stats.

4. **Recovery** (`lifecycle.rs`): In `reconcile_state()`, iterate `state.agent_runs` for non-terminal runs:
   - Check tmux session liveness (same pattern as pipeline recovery)
   - If alive → reconnect monitoring via `recover_standalone_agent()`
   - If dead → emit `AgentRunStatusChanged { status: Failed }`

5. **Agent send**: Extend `AgentSend` handler to also resolve standalone agent IDs.

**Verification:** `cargo test -p oj-daemon` — integration test: send `RunCommand` with agent directive, verify `AgentRunStarted` response.

### Phase 5: CLI Dispatch + Agent Display

Update CLI to handle standalone agent responses and display.

**Files:**

- `crates/cli/src/commands/run.rs` (modify)
- `crates/cli/src/commands/agent.rs` (modify)
- `crates/daemon/src/protocol.rs` (verify AgentSummary fields)

**Details:**

1. **CLI run dispatch** (`run.rs`): The current `dispatch_to_daemon()` always expects `CommandStarted { pipeline_id, pipeline_name }`. When the command is an agent directive, the daemon returns `AgentRunStarted { agent_run_id, agent_name }`. Update the client:

```rust
// In dispatch_to_daemon, after sending RunCommand:
match response {
    Response::CommandStarted { pipeline_id, pipeline_name } => {
        // existing pipeline flow
    }
    Response::AgentRunStarted { agent_run_id, agent_name } => {
        let short_id = &agent_run_id[..8.min(agent_run_id.len())];
        println!("Agent: {agent_name} ({short_id})");
        println!();
        println!("  oj agent show {short_id}");
        println!("  oj agent logs {short_id}");
    }
    // ...
}
```

2. **Agent list** (`agent.rs`): The `ListAgents` query handler now returns both pipeline agents and standalone agents in one list. The CLI table rendering should handle `pipeline_id: ""` by showing `"—"` in the pipeline column. Sort by `updated_at_ms` descending (most recent first).

3. **Agent show**: Same — `AgentDetail` for standalone agents has empty `pipeline_id`/`step_name`. Display `"standalone"` for the source field.

4. **Agent logs**: For standalone agents, logs are stored under `{logs}/agent/{agent_id}/` (same structure). The `GetAgentLogs` query needs to handle lookup by `agent_run_id` → `agent_id` mapping.

5. **Agent wait**: Extend `handle_wait()` to also resolve standalone agents. Terminal conditions: `Completed` → exit 0, `Failed` → exit 1, `Escalated` → exit 1.

**Verification:** Manual testing with a test runbook:

```hcl
agent "greeter" {
  run   = "claude --model haiku"
  prompt = "Say hello and then use oj_signal to signal done."
  on_idle { action = "done" }
  on_dead { action = "done" }
}

command "greet" {
  run = { agent = "greeter" }
}
```

Run `oj run greet`, verify agent appears in `oj agent list`, verify `oj agent show`, verify `oj agent logs`.

### Phase 6: Agent Prune, Cancel, and Hook Support

Handle operational commands for standalone agents.

**Files:**

- `crates/daemon/src/protocol.rs` (modify — cancel/prune)
- `crates/engine/src/runtime/agent_run.rs` (modify — cancel)
- `crates/cli/src/commands/agent.rs` (modify — prune, hook)
- `crates/daemon/src/listener/` (modify)

**Details:**

1. **Cancel**: Add `Request::AgentRunCancel { id: String }` or reuse the agent signal mechanism. Cancelling a standalone agent kills the tmux session and emits `AgentRunStatusChanged { status: Failed, reason: "cancelled" }`.

2. **Prune**: Extend `AgentPrune` to also clean up terminal standalone agent runs from `MaterializedState.agent_runs` and their log files.

3. **Hook support**: The stop hook (`oj agent hook stop`) queries `GetAgentSignal`. For standalone agents, the signal is stored on `AgentRun.agent_signal` instead of `Pipeline.agent_signal`. Extend the query handler.

4. **Resume**: For escalated standalone agents, `oj pipeline resume` doesn't apply. Add `Request::AgentRunResume { id, message }` that re-triggers the lifecycle (re-spawns the agent or sends a nudge).

**Verification:** `cargo test --all` + `make check` — all tests pass, no clippy warnings.

## Key Implementation Details

### Agent Routing

The critical routing decision happens when `AgentStateChanged` arrives in the event loop. Today, the flow is:

```
AgentStateChanged { agent_id } →
  agent_pipelines.get(agent_id) →
    pipeline.step matches → handle_monitor_state(pipeline, ...)
```

After this change:

```
AgentStateChanged { agent_id } →
  1. agent_runs.get(agent_id) → handle_standalone_monitor_state(agent_run, ...)
  2. agent_pipelines.get(agent_id) → handle_monitor_state(pipeline, ...)
  3. neither → warn("unknown agent")
```

### Liveness Timer Keying

Today, liveness timers use `TimerId::liveness(&pipeline_id)`. For standalone agents, use `TimerId::liveness_agent_run(&agent_run_id)` (new variant or namespace the string differently). The timer handler must check both maps.

### Logging

Standalone agent logs go to `{logs}/agent/{agent_id}/` (same as pipeline agents). The `AgentLogger` already supports this — it just needs the agent_run_id→agent_id mapping to resolve log queries by run ID.

### WAL Backward Compatibility

New event variants use `#[serde(rename = "agent_run:*")]` tags. Old daemons encountering these events will deserialize them as `Event::Custom` (the catch-all), which is a no-op in `apply_event`. This is safe.

### No Synthetic Pipeline

The design explicitly avoids creating a synthetic single-step pipeline. This means:
- `oj pipeline list` does NOT show standalone agents
- `oj agent list` DOES show standalone agents
- `oj pipeline cancel` does NOT cancel standalone agents (use `oj agent cancel` or a new mechanism)
- Pipeline-specific features (vars, step history, workspace isolation) don't apply

## Verification Plan

1. **Unit tests** (each phase):
   - `oj-core`: AgentRun state transitions, serialization roundtrips
   - `oj-storage`: Event materialization for all AgentRun events
   - `oj-engine`: Spawn context abstraction, standalone lifecycle actions, agent routing

2. **Integration tests** (Phase 4-5):
   - Send `RunCommand` with agent directive → verify `AgentRunStarted` response
   - Agent state changes → verify `AgentRunStatusChanged` events in WAL
   - `ListAgents` returns both pipeline and standalone agents
   - `GetAgent` resolves standalone agents by ID prefix

3. **End-to-end test** (Phase 5):
   - Create test runbook with agent command
   - `oj run greet` → verify agent spawns in tmux
   - `oj agent list` → verify agent appears
   - `oj agent show <id>` → verify details
   - Agent completes → verify `Completed` status
   - `oj agent logs <id>` → verify logs accessible

4. **Recovery test** (Phase 4):
   - Spawn standalone agent, kill daemon, restart
   - Verify agent is recovered and monitoring reconnected

5. **Full suite**: `make check` passes at each phase boundary.
