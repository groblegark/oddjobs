# Plan: `oj agent send`

## Overview

Add an `oj agent send <agent-id> <message>` CLI command that sends a text message to a running agent. The message is delivered through the agent adapter (currently tmux), as if a human typed it and pressed Enter. The agent-id can be an agent UUID, a pipeline ID, or a prefix of either.

This builds on the existing `oj session send` infrastructure but routes through the agent layer instead of the session layer, providing proper agent-level abstractions and reliable message delivery with correct Enter-key timing.

## Project Structure

Files to create or modify:

```
crates/
├── daemon/src/
│   ├── protocol.rs          # Add Request::AgentSend variant
│   └── listener/
│       └── mod.rs            # Route AgentSend to handler
│       └── mutations.rs      # Add handle_agent_send handler
├── cli/src/
│   ├── commands/agent.rs     # Add Send subcommand
│   ├── client.rs             # Add agent_send client method
│   └── main.rs               # Update Agent dispatch (action semantics for Send)
├── adapters/src/
│   ├── session/
│   │   ├── mod.rs            # Add send_literal + send_enter to SessionAdapter trait
│   │   ├── tmux.rs           # Implement literal send + Enter for TmuxAdapter
│   │   ├── noop.rs           # Stub implementations
│   │   └── fake.rs           # Test support implementations
│   └── agent/
│       ├── claude.rs         # Update send() to use send_literal + send_enter
│       └── fake.rs           # Update FakeAgentAdapter
├── core/src/
│   └── event.rs              # Add Event::AgentInput variant
└── engine/src/
    └── runtime/handlers/
        └── mod.rs            # Handle AgentInput event → Effect::SendToAgent
```

## Dependencies

No new external dependencies. All changes use existing crates (clap, serde, tokio, async-trait).

## Implementation Phases

### Phase 1: Reliable tmux message delivery (SessionAdapter)

The current `SessionAdapter::send()` uses `tmux send-keys` without `-l` (literal mode). This means special characters and the Enter key can race. Add two new methods to the `SessionAdapter` trait for reliable text + Enter delivery.

**Changes:**

1. **`crates/adapters/src/session/mod.rs`** — Add two new methods to the `SessionAdapter` trait:

```rust
/// Send literal text to a session (no key interpretation)
async fn send_literal(&self, id: &str, text: &str) -> Result<(), SessionError>;

/// Send the Enter key to a session
async fn send_enter(&self, id: &str) -> Result<(), SessionError>;
```

2. **`crates/adapters/src/session/tmux.rs`** — Implement for `TmuxAdapter`:

```rust
async fn send_literal(&self, id: &str, text: &str) -> Result<(), SessionError> {
    // tmux send-keys -t {id} -l -- {text}
    // -l = literal mode (no key name interpretation)
    // -- = end of options (handles text starting with -)
    let output = Command::new("tmux")
        .args(["send-keys", "-t", id, "-l", "--", text])
        .output()
        .await
        .map_err(|e| SessionError::CommandFailed(e.to_string()))?;

    if !output.status.success() {
        return Err(SessionError::NotFound(id.to_string()));
    }
    Ok(())
}

async fn send_enter(&self, id: &str) -> Result<(), SessionError> {
    // tmux send-keys -t {id} Enter
    let output = Command::new("tmux")
        .args(["send-keys", "-t", id, "Enter"])
        .output()
        .await
        .map_err(|e| SessionError::CommandFailed(e.to_string()))?;

    if !output.status.success() {
        return Err(SessionError::NotFound(id.to_string()));
    }
    Ok(())
}
```

3. **`crates/adapters/src/session/noop.rs`** — Add no-op stubs for both methods.

4. **`crates/adapters/src/session/fake.rs`** — Add tracking for both methods (record calls for test assertions).

**Verification:** `cargo test -p oj-adapters` passes. Existing session tests still pass.

### Phase 2: Update AgentAdapter send to use reliable delivery

Update `ClaudeAgentAdapter::send()` to use the new `send_literal` + `send_enter` methods instead of the old `SessionAdapter::send()`. This ensures all agent messages (including nudges from the engine) benefit from reliable delivery.

**Changes:**

1. **`crates/adapters/src/agent/claude.rs`** — Update `send()` implementation:

```rust
async fn send(&self, agent_id: &AgentId, input: &str) -> Result<(), AgentError> {
    let session_id = {
        let agents = self.agents.lock();
        agents
            .get(agent_id)
            .map(|info| info.session_id.clone())
            .ok_or_else(|| AgentError::NotFound(agent_id.to_string()))?
    };

    // Send literal text first, then Enter separately for reliability
    self.sessions
        .send_literal(&session_id, input)
        .await
        .map_err(|e| AgentError::SendFailed(e.to_string()))?;

    self.sessions
        .send_enter(&session_id)
        .await
        .map_err(|e| AgentError::SendFailed(e.to_string()))
}
```

2. **`crates/adapters/src/agent/fake.rs`** — Ensure `FakeAgentAdapter::send()` records calls properly (likely no changes needed).

**Verification:** `cargo test -p oj-adapters` and `cargo test -p oj-engine` pass.

### Phase 3: Protocol and event plumbing

Add the `AgentSend` request to the protocol and the `AgentInput` event to the core event system.

**Changes:**

1. **`crates/daemon/src/protocol.rs`** — Add variant to `Request`:

```rust
/// Send input to an agent
AgentSend { agent_id: String, message: String },
```

2. **`crates/core/src/event.rs`** — Add variant to `Event`:

```rust
/// User-initiated input to an agent (resolved by daemon to the active agent)
AgentInput { agent_id: AgentId, input: String },
```

3. **`crates/engine/src/runtime/handlers/mod.rs`** — Handle the new event:

```rust
Event::AgentInput { agent_id, input } => {
    self.executor
        .execute(Effect::SendToAgent {
            agent_id: agent_id.clone(),
            input: input.clone(),
        })
        .await?;
}
```

Note: The `Effect::SendToAgent` and its executor handler already exist (`crates/engine/src/executor.rs:157`). The executor calls `self.agents.send(&agent_id, &input)` which routes through the `AgentAdapter` trait.

4. **`crates/storage/src/state.rs`** — Add a no-op match arm for `Event::AgentInput` in `apply_event()` (this event has no state side-effects, it's purely an effect trigger).

**Verification:** `cargo test --all` passes. Protocol serialization round-trips correctly.

### Phase 4: Daemon handler for AgentSend

Add the daemon-side handler that resolves the agent ID and emits the `AgentInput` event.

**Changes:**

1. **`crates/daemon/src/listener/mutations.rs`** — Add handler:

```rust
/// Handle an agent send request.
///
/// Resolves agent_id via:
/// 1. Direct match on pipeline agent_id (from step_history)
/// 2. Pipeline ID lookup → current step's agent_id
/// 3. Prefix match on either
pub(super) fn handle_agent_send(
    state: &Arc<Mutex<MaterializedState>>,
    event_bus: &EventBus,
    agent_id: String,
    message: String,
) -> Result<Response, ConnectionError> {
    let resolved_agent_id = {
        let state_guard = state.lock();

        // 1. Check if any pipeline has an agent with this exact ID or prefix
        let mut found: Option<String> = None;
        for pipeline in state_guard.pipelines.values() {
            if let Some(record) = pipeline.step_history.last() {
                if let Some(aid) = &record.agent_id {
                    if aid == &agent_id || aid.starts_with(&agent_id) {
                        found = Some(aid.clone());
                        break;
                    }
                }
            }
        }

        // 2. If not found by agent_id, try as pipeline ID → active agent
        if found.is_none() {
            if let Some(pipeline) = state_guard.get_pipeline(&agent_id) {
                if let Some(record) = pipeline.step_history.last() {
                    found = record.agent_id.clone();
                }
            }
        }

        found
    };

    match resolved_agent_id {
        Some(aid) => {
            let event = Event::AgentInput {
                agent_id: AgentId::new(aid),
                input: message,
            };
            event_bus
                .send(event)
                .map_err(|_| ConnectionError::WalError)?;
            Ok(Response::Ok)
        }
        None => Ok(Response::Error {
            message: format!("Agent not found: {}", agent_id),
        }),
    }
}
```

2. **`crates/daemon/src/listener/mod.rs`** — Route the request:

```rust
Request::AgentSend { agent_id, message } => {
    mutations::handle_agent_send(state, event_bus, agent_id, message)
}
```

**Verification:** `cargo test -p oj-daemon` passes.

### Phase 5: CLI command and client wiring

Add the `oj agent send` CLI command and wire it through the client.

**Changes:**

1. **`crates/cli/src/client.rs`** — Add client method:

```rust
/// Send a message to a running agent
pub async fn agent_send(&self, agent_id: &str, message: &str) -> Result<(), ClientError> {
    let request = Request::AgentSend {
        agent_id: agent_id.to_string(),
        message: message.to_string(),
    };
    self.send_simple(&request).await
}
```

2. **`crates/cli/src/commands/agent.rs`** — Add `Send` variant to `AgentCommand`:

```rust
#[derive(Subcommand)]
pub enum AgentCommand {
    /// Send a message to a running agent
    Send {
        /// Agent ID or pipeline ID (or prefix)
        agent_id: String,
        /// Message to send
        message: String,
    },
    // ... existing variants
}
```

Handle it:

```rust
AgentCommand::Send { agent_id, message } => {
    client.agent_send(&agent_id, &message).await?;
    println!("Sent to agent {}", agent_id);
}
```

3. **`crates/cli/src/main.rs`** — Update the `Agent` dispatch to use action semantics for `Send` (since it mutates agent state) and query semantics for the rest:

```rust
Commands::Agent(args) => {
    use agent::AgentCommand;
    match &args.command {
        AgentCommand::Send { .. } => {
            let client = DaemonClient::for_action()?;
            agent::handle(args.command, &client, format).await?
        }
        _ => {
            let client = DaemonClient::for_query()?;
            agent::handle(args.command, &client, format).await?
        }
    }
}
```

**Verification:** `cargo build --all` succeeds. Manual test: `oj agent send <id> "hello"` sends message to running agent.

### Phase 6: Tests and final verification

Add unit tests and run the full check suite.

**Changes:**

1. **`crates/daemon/src/protocol_tests.rs`** — Add serialization round-trip test for `Request::AgentSend`.

2. **`crates/adapters/src/session/tmux_tests.rs`** — Add tests for `send_literal` and `send_enter` (if there are existing tmux unit tests; otherwise skip since these require tmux).

3. **`crates/daemon/src/listener_tests.rs`** — Add test for `handle_agent_send` with:
   - Direct agent ID match
   - Pipeline ID → agent resolution
   - Prefix match
   - Not-found error case

4. Run `make check`:
   - `cargo fmt --all -- --check`
   - `cargo clippy --all-targets --all-features -- -D warnings`
   - `quench check`
   - `cargo test --all`
   - `cargo build --all`
   - `cargo audit`
   - `cargo deny check licenses bans sources`

## Key Implementation Details

### ID Resolution Strategy

The `handle_agent_send` handler resolves `agent_id` using three strategies:

1. **Agent ID match**: Scan all pipelines' `step_history` for an agent with a matching `agent_id` (exact or prefix)
2. **Pipeline ID match**: Look up by pipeline ID, then get the current step's `agent_id`
3. **Prefix match**: Both strategies support prefix matching (like git commit hashes)

This mirrors the pattern in `find_agent()` in `crates/cli/src/commands/agent.rs:177-192`, but done server-side for efficiency.

### Reliable tmux Message Delivery

The key constraint is that text and Enter must not race. The solution:

1. `tmux send-keys -t {session} -l -- {text}` — sends text literally (no key interpretation)
2. `tmux send-keys -t {session} Enter` — sends the Enter key

These are two separate tmux commands executed sequentially. The `-l` flag ensures that text containing special tmux key names (like "Enter", "Escape", etc.) is sent as literal text. The `--` prevents text starting with `-` from being interpreted as flags.

### Event Flow

```
CLI (oj agent send)
  → DaemonClient::agent_send()
    → Request::AgentSend { agent_id, message }
      → Listener::handle_agent_send()
        → resolves agent_id
        → EventBus::send(Event::AgentInput { agent_id, input })
          → Engine handler
            → Effect::SendToAgent { agent_id, input }
              → AgentAdapter::send()
                → SessionAdapter::send_literal() + send_enter()
                  → tmux send-keys -l + tmux send-keys Enter
```

### Why AgentInput Event (Not Direct Adapter Call)

The daemon listener doesn't have direct access to the `AgentAdapter`. All mutations flow through the event bus → engine → executor pattern. This maintains the functional core / imperative shell architecture. The `Effect::SendToAgent` executor handler already exists and handles the adapter call.

### Client Semantics

`oj agent send` uses `DaemonClient::for_action()` (auto-start daemon) because:
- It's user-initiated (not agent-initiated)
- It mutates state (sends input to an agent)
- The user expects the daemon to be running; auto-start is helpful

## Verification Plan

1. **Unit tests**: Protocol round-trip, handler resolution logic, fake adapter call recording
2. **Integration test (manual)**:
   - Start a pipeline with an agent step
   - Run `oj agent send <pipeline-id> "hello"` → message appears in agent's tmux session
   - Run `oj agent send <agent-uuid-prefix> "hello"` → same result
   - Run `oj agent send nonexistent "hello"` → "Agent not found" error
3. **Full suite**: `make check` passes with no warnings or failures
