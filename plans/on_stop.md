# Plan: `on_stop` Lifecycle Handler for Agents

## Overview

Add a configurable `on_stop` lifecycle handler to agent definitions that controls what happens when an agent tries to exit (Claude Code's Stop hook fires). Three actions are supported:

- **`signal`** — Block exit until the agent explicitly calls `oj emit agent:signal` (current hardcoded behavior)
- **`idle`** — Treat the stop attempt as an idle event, firing the agent's `on_idle` handler
- **`escalate`** — Block exit and immediately escalate to a human (notification + waiting status)

Defaults differ by context:
- **Pipeline agents**: `signal` (require explicit signaling before exit)
- **Standalone agents**: `escalate` (notify human immediately when agent tries to stop)

## Project Structure

Files created or modified:

```
crates/
├── runbook/src/
│   ├── agent.rs            # Add StopAction, StopActionConfig types; add on_stop to AgentDef
│   └── agent_tests.rs      # Unit tests for new types and parsing
├── core/src/
│   └── event.rs            # Add Event::AgentStop variant
├── engine/src/
│   ├── workspace.rs         # Write on_stop config to agent state dir at spawn time
│   ├── spawn.rs             # Pass on_stop to workspace setup
│   ├── runtime/
│   │   ├── handlers/
│   │   │   ├── mod.rs       # Route Event::AgentStop to handler
│   │   │   └── agent.rs     # Add handle_agent_stop_hook()
│   │   ├── monitor.rs       # Add handle for standalone agent stop
│   │   └── agent_run.rs     # Add standalone agent stop handling
│   └── monitor.rs           # (no changes — on_stop uses different action set)
├── cli/src/commands/
│   └── agent.rs             # Update handle_stop_hook() to read on_stop config
├── daemon/src/
│   └── listener/query.rs    # Fix orphaned agent signal query (allow exit when pipeline advanced)
docs/concepts/
│   └── RUNBOOKS.md          # Document on_stop field
tests/specs/daemon/
│   └── on_stop.rs           # Integration tests (new file)
tests/
│   └── specs.rs             # Register on_stop test module
```

## Dependencies

No new external dependencies. All changes use existing crates and patterns.

## Implementation Phases

### Phase 1: Types and Parsing

Add the `on_stop` types and integrate them into the agent definition.

**1a. Define `StopAction` and `StopActionConfig` in `crates/runbook/src/agent.rs`:**

```rust
/// What to do when the agent's Stop hook fires (agent tries to exit)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum StopAction {
    /// Block exit until agent calls `oj emit agent:signal` (pipeline default)
    Signal,
    /// Treat stop as idle — fire on_idle handler
    Idle,
    /// Block exit and escalate to human (standalone default)
    Escalate,
}

/// Configuration for the on_stop lifecycle handler
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum StopActionConfig {
    Simple(StopAction),
    WithOptions {
        action: StopAction,
    },
}

impl StopActionConfig {
    pub fn action(&self) -> &StopAction {
        match self {
            StopActionConfig::Simple(a) => a,
            StopActionConfig::WithOptions { action } => action,
        }
    }
}
```

**1b. Add `on_stop` field to `AgentDef`:**

```rust
pub struct AgentDef {
    // ... existing fields ...

    /// What to do when agent tries to exit (Stop hook).
    /// None = context-dependent default (pipeline: signal, standalone: escalate)
    #[serde(default)]
    pub on_stop: Option<StopActionConfig>,

    // ... remaining fields ...
}
```

Also update the `Default` impl for `AgentDef` to set `on_stop: None`.

**1c. Add unit tests** in `agent_tests.rs`:
- Parse `on_stop = "signal"` (simple form)
- Parse `on_stop = { action = "idle" }` (object form)
- Parse `on_stop = "escalate"`
- Default (`None`) when omitted
- Reject invalid values (e.g., `on_stop = "nudge"`)

**Milestone:** `cargo test -p oj-runbook` passes with new types.

---

### Phase 2: Write Config at Spawn Time

Make the resolved `on_stop` action available to the CLI stop hook handler at runtime by writing it to the agent's state directory during spawn.

**2a. Add config write function in `crates/engine/src/workspace.rs`:**

```rust
/// Write agent runtime config (on_stop action) to the agent state directory.
///
/// The CLI stop hook reads this file to determine behavior.
pub fn write_agent_config(
    agent_id: &str,
    on_stop: &str,
    state_dir: &Path,
) -> io::Result<()> {
    let agent_dir = agent_state_dir(agent_id, state_dir)?;
    let config = serde_json::json!({ "on_stop": on_stop });
    fs::write(
        agent_dir.join("config.json"),
        serde_json::to_string(&config).unwrap_or_default(),
    )
}
```

**2b. Call from spawn code in `crates/engine/src/spawn.rs`:**

When building spawn effects, resolve the on_stop default based on context and write the config file:

```rust
// Resolve on_stop: explicit config, or context-dependent default
let on_stop_action = agent_def.on_stop
    .as_ref()
    .map(|c| c.action())
    .cloned()
    .unwrap_or(if is_standalone { StopAction::Escalate } else { StopAction::Signal });

let on_stop_str = match on_stop_action {
    StopAction::Signal => "signal",
    StopAction::Idle => "idle",
    StopAction::Escalate => "escalate",
};

workspace::write_agent_config(agent_id_str, on_stop_str, state_dir)?;
```

The spawn context already distinguishes pipeline vs standalone agents (the caller knows which path it's taking). Pass an `is_standalone` flag through `SpawnContext` or as a parameter.

**2c. Also write config for standalone agent runs** in the standalone agent spawn path (`crates/engine/src/runtime/agent_run.rs`).

**Milestone:** After spawning an agent, `{state_dir}/agents/{agent_id}/config.json` exists with the resolved on_stop action.

---

### Phase 3: Update Stop Hook Handler

Modify the CLI stop hook to read the on_stop config and behave accordingly.

**3a. Read on_stop config in `crates/cli/src/commands/agent.rs` `handle_stop_hook()`:**

```rust
async fn handle_stop_hook(agent_id: &str, client: &DaemonClient) -> Result<()> {
    let mut input_json = String::new();
    io::stdin().read_to_string(&mut input_json)?;

    let input: StopHookInput = serde_json::from_str(&input_json)
        .unwrap_or(StopHookInput { stop_hook_active: false });

    // Prevent infinite loops
    if input.stop_hook_active {
        std::process::exit(0);
    }

    // Read on_stop config from agent state dir
    let on_stop = read_on_stop_config(agent_id);

    // Query daemon: has this agent signaled completion?
    let response = client.query_agent_signal(agent_id).await?;

    if response.signaled {
        // Agent has signaled — allow exit regardless of on_stop
        std::process::exit(0);
    }

    match on_stop.as_str() {
        "idle" => {
            // Emit idle event, then block
            let event = Event::AgentIdle {
                agent_id: AgentId::new(agent_id),
            };
            let _ = client.emit_event(event).await;
            block_exit(agent_id, "Stop hook: on_idle handler invoked. Continue working or signal completion.");
        }
        "escalate" => {
            // Emit stop event for escalation, then block
            let event = Event::AgentStop {
                agent_id: AgentId::new(agent_id),
            };
            let _ = client.emit_event(event).await;
            block_exit(agent_id, "A human has been notified. Wait for instructions or signal completion.");
        }
        _ => {
            // "signal" (default) — current behavior
            block_exit(agent_id, &format!(
                "You must explicitly signal completion before stopping. \
                 Run: oj emit agent:signal --agent {} '<json>' ...",
                agent_id
            ));
        }
    }
}

fn read_on_stop_config(agent_id: &str) -> String {
    let state_dir = match std::env::var("OJ_STATE_DIR") {
        Ok(d) => PathBuf::from(d),
        Err(_) => return "signal".to_string(),
    };
    let config_path = state_dir.join("agents").join(agent_id).join("config.json");
    std::fs::read_to_string(&config_path)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("on_stop")?.as_str().map(String::from))
        .unwrap_or_else(|| "signal".to_string())
}

fn block_exit(agent_id: &str, reason: &str) {
    let output = StopHookOutput {
        decision: "block".to_string(),
        reason: reason.to_string(),
    };
    let output_json = serde_json::to_string(&output).unwrap();
    io::stdout().write_all(output_json.as_bytes()).unwrap();
    io::stdout().flush().unwrap();
    std::process::exit(0);
}
```

**Milestone:** Stop hook reads config and behaves differently based on action. `idle` emits `agent:idle`, `escalate` emits `agent:stop`, `signal` blocks with instructions (current behavior).

---

### Phase 4: Agent Stop Event and Escalation Handler

Add a new `Event::AgentStop` for the escalation path and wire up the daemon handler.

**4a. Add event variant in `crates/core/src/event.rs`:**

```rust
/// Agent stop hook fired with on_stop=escalate (from CLI hook)
#[serde(rename = "agent:stop")]
AgentStop { agent_id: AgentId },
```

Update `Event::name()`, `Event::log_summary()`, and any other match arms.

**4b. Route event in `crates/engine/src/runtime/handlers/mod.rs`:**

```rust
Event::AgentStop { agent_id } => {
    result_events.extend(
        self.handle_agent_stop_hook(agent_id).await?,
    );
}
```

**4c. Implement `handle_agent_stop_hook()` in `crates/engine/src/runtime/handlers/agent.rs`:**

```rust
/// Handle agent:stop — fired when on_stop=escalate and agent tries to exit.
///
/// Escalates to human: sends notification and sets pipeline/agent_run to waiting.
/// Idempotent: skips if already in waiting status.
pub(crate) async fn handle_agent_stop_hook(
    &self,
    agent_id: &AgentId,
) -> Result<Vec<Event>, RuntimeError> {
    // Check standalone agent runs first
    let maybe_run_id = { self.agent_runs.lock().get(agent_id).cloned() };
    if let Some(agent_run_id) = maybe_run_id {
        let agent_run = self.lock_state(|s| s.agent_runs.get(agent_run_id.as_str()).cloned());
        if let Some(agent_run) = agent_run {
            if agent_run.status.is_terminal()
                || agent_run.status == AgentRunStatus::Escalated
            {
                return Ok(vec![]);
            }
            // Fire standalone escalation
            let effects = vec![
                Effect::Notify {
                    title: format!("Agent needs attention: {}", agent_run.command_name),
                    message: "Agent tried to stop without signaling completion".to_string(),
                },
                Effect::Emit {
                    event: Event::AgentRunStatusChanged {
                        id: AgentRunId::new(&agent_run.id),
                        status: AgentRunStatus::Escalated,
                        reason: Some("on_stop: escalate".to_string()),
                    },
                },
            ];
            return Ok(self.executor.execute_all(effects).await?);
        }
    }

    // Pipeline agent
    let Some(pipeline_id_str) = self.agent_pipelines.lock().get(agent_id).cloned() else {
        return Ok(vec![]);
    };
    let pipeline = self.require_pipeline(&pipeline_id_str)?;

    if pipeline.is_terminal() || pipeline.step_status == StepStatus::Waiting {
        return Ok(vec![]);  // Already escalated or terminal — no-op
    }

    // Stale agent check
    let current_agent_id = pipeline
        .step_history.iter()
        .rfind(|r| r.name == pipeline.step)
        .and_then(|r| r.agent_id.as_deref());
    if current_agent_id != Some(agent_id.as_str()) {
        return Ok(vec![]);
    }

    let pipeline_id = PipelineId::new(&pipeline.id);
    let effects = vec![
        Effect::Notify {
            title: format!("Pipeline needs attention: {}", pipeline.name),
            message: "Agent tried to stop without signaling completion".to_string(),
        },
        Effect::Emit {
            event: Event::StepWaiting {
                pipeline_id: pipeline_id.clone(),
                step: pipeline.step.clone(),
                reason: Some("on_stop: escalate".to_string()),
                decision_id: None,
            },
        },
        Effect::CancelTimer {
            id: TimerId::exit_deferred(&pipeline_id),
        },
    ];
    Ok(self.executor.execute_all(effects).await?)
}
```

**4d. Fix orphaned agent in `GetAgentSignal` query** (`crates/daemon/src/listener/query.rs`):

When no pipeline or agent_run currently owns the agent (pipeline has advanced past it), return `signaled: true` to allow exit. This prevents agents from getting stuck after their pipeline advances:

```rust
Query::GetAgentSignal { agent_id } => {
    // Check standalone agent runs
    // ... (existing logic) ...

    // Check pipelines
    let pipeline_signal = state.pipelines.values().find_map(|p| {
        let matches = p.step_history.iter()
            .rfind(|r| r.name == p.step)
            .and_then(|r| r.agent_id.as_deref())
            == Some(&agent_id);
        if matches { Some(p.agent_signal.as_ref()) } else { None }
    });

    match pipeline_signal {
        Some(Some(s)) => Response::AgentSignal { signaled: true, ... },
        Some(None) => Response::AgentSignal { signaled: false, ... },
        None => {
            // No pipeline or agent_run owns this agent — orphaned or pipeline advanced.
            // Allow exit to prevent the agent from getting stuck.
            Response::AgentSignal { signaled: true, kind: None, message: None }
        }
    }
}
```

This is important for `on_stop = idle` with `on_idle = done`: the pipeline advances, the agent tries to stop again, and the query now returns `signaled: true` because the pipeline no longer references this agent.

**Milestone:** `Event::AgentStop` is handled in the runtime. Escalation fires notification and sets waiting status. Orphaned agents can exit.

---

### Phase 5: Tests and Documentation

**5a. Unit tests in `crates/runbook/src/agent_tests.rs`:**
- Parsing all three on_stop actions (simple and object forms)
- Default (None) when omitted
- Invalid action values rejected

**5b. Integration tests in `tests/specs/daemon/on_stop.rs`:**

Register in `tests/specs.rs`:
```rust
#[path = "specs/daemon/on_stop.rs"]
mod daemon_on_stop;
```

Test scenarios:

1. **`on_stop = "signal"` (default for pipeline):** Agent must signal before exit. Current behavior preserved.

2. **`on_stop = "idle"` with `on_idle = "done"`:** Agent tries to stop → idle event fires → pipeline advances → agent exits on next stop attempt.

3. **`on_stop = "idle"` with `on_idle = "nudge"`:** Agent tries to stop → idle event fires → nudge sent → agent continues working.

4. **`on_stop = "escalate"` (default for standalone):** Agent tries to stop → escalation notification → pipeline enters waiting status → human intervenes.

5. **Standalone agent default:** Run a standalone agent with no `on_stop` config. Verify escalation fires when agent tries to stop.

6. **Pipeline agent default:** Run a pipeline agent with no `on_stop` config. Verify signal behavior (current behavior).

All tests use `claudeless` with appropriate scenarios and `wait_for()` polling (no sleeps).

**5c. Update `docs/concepts/RUNBOOKS.md`:**

Add `on_stop` to the agent fields documentation:

```markdown
- **on_stop**: What to do when agent tries to exit (default: `"signal"` for pipeline, `"escalate"` for standalone)
```

Add to the valid actions table:

```markdown
- **on_stop**: `signal`, `idle`, `escalate`
```

Add brief explanation of each action.

**Milestone:** All tests pass. `make check` clean. Documentation updated.

## Key Implementation Details

### Why File-Based Config (Not Daemon Query)

The on_stop action is written to `{state_dir}/agents/{agent_id}/config.json` at spawn time rather than queried from the daemon because:

1. **No protocol changes needed** — the existing `GetAgentSignal` query is unchanged
2. **No WAL/state schema changes** — no new fields on Pipeline or AgentRun
3. **Simple backward compat** — missing file defaults to "signal" (current behavior)
4. **Already-available state dir** — the CLI stop hook already reads `OJ_STATE_DIR` for logging

### Signal Check Always Comes First

Regardless of `on_stop` action, if the agent has already called `oj emit agent:signal`, the stop hook allows exit. The signal is authoritative — the agent explicitly communicated its intent.

### Idle Re-entrancy

When `on_stop = idle`, each stop attempt emits a fresh `agent:idle` event. The daemon's existing attempt tracking (`action_attempts` on Pipeline/AgentRun) prevents unbounded loops:

1. Agent tries to stop → idle event → on_idle fires (attempt 1)
2. If nudged, agent works more, tries to stop → idle event → on_idle fires (attempt 2)
3. After `attempts` exhausted → auto-escalates

This is consistent with how `on_idle` already handles repeated idle events from the Notification hook.

### Escalation Idempotency

The `handle_agent_stop_hook()` handler skips escalation if the pipeline is already in `Waiting` status or the agent_run is already `Escalated`. This prevents duplicate notifications when the stop hook fires multiple times.

### Orphaned Agent Fix

When a pipeline advances past an agent step (e.g., `on_idle = done` advanced it), the agent is no longer referenced by any active pipeline step. The `GetAgentSignal` query now returns `signaled: true` for orphaned agents, allowing them to exit gracefully. Without this fix, `on_stop = idle` with `on_idle = done` would leave agents stuck.

## Verification Plan

1. **Unit tests:** `cargo test -p oj-runbook` — parsing and validation of on_stop types
2. **Build check:** `cargo build --all` — compilation with new types and event variant
3. **Lint:** `cargo clippy --all -- -D warnings` — no new warnings
4. **Integration tests:** `cargo test --test specs daemon_on_stop` — behavioral verification
5. **Full check:** `make check` — complete CI verification
6. **Manual verification:** Spawn a pipeline agent and standalone agent, observe stop hook behavior matches configured action
