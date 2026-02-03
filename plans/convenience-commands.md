# Convenience Commands

## Overview

Add four top-level convenience commands — `oj peek <id>`, `oj attach <id>`, `oj logs <id>`, `oj show <id>` — that resolve an ID across all entity types (pipeline, agent, session) and dispatch to the appropriate typed subcommand. This eliminates the need for users to remember which entity type an ID belongs to.

## Project Structure

```
crates/cli/src/
├── main.rs                  # Add new Commands variants + dispatch logic
├── commands/
│   ├── mod.rs               # Add `pub mod resolve;`
│   └── resolve.rs           # NEW: Cross-entity ID resolution + convenience command handlers
├── client.rs                # Add resolve_entity() method
```

## Dependencies

No new external dependencies. Uses existing `DaemonClient` methods (`list_pipelines`, `get_pipeline`, `list_agents`, `list_sessions`).

## Implementation Phases

### Phase 1: Entity Resolution

Add a module `crates/cli/src/commands/resolve.rs` with cross-entity ID lookup logic.

**Entity types to search (in order):**
1. Pipelines — `client.list_pipelines()` → match on `id` field
2. Agents — `client.list_agents(None, None)` → match on `agent_id` field
3. Sessions — `client.list_sessions()` → match on `id` field

**Resolution rules:**
- Try exact match first across all entity types
- Then try prefix match (ID starts with the query)
- Collect all matches into a `Vec<EntityMatch>`

```rust
pub enum EntityKind {
    Pipeline,
    Agent,
    Session,
}

pub struct EntityMatch {
    pub kind: EntityKind,
    pub id: String,
    /// Human-readable label (e.g. pipeline name, agent step name)
    pub label: Option<String>,
}

/// Resolve an ID across all entity types.
/// Returns all matches (exact matches take priority over prefix matches).
pub async fn resolve_entity(client: &DaemonClient, query: &str) -> Result<Vec<EntityMatch>> {
    let mut exact = Vec::new();
    let mut prefix = Vec::new();

    // Check pipelines
    let pipelines = client.list_pipelines().await?;
    for p in &pipelines {
        if p.id == query {
            exact.push(EntityMatch { kind: EntityKind::Pipeline, id: p.id.clone(), label: Some(p.name.clone()) });
        } else if p.id.starts_with(query) {
            prefix.push(EntityMatch { kind: EntityKind::Pipeline, id: p.id.clone(), label: Some(p.name.clone()) });
        }
    }

    // Check agents
    let agents = client.list_agents(None, None).await?;
    for a in &agents {
        if a.agent_id == query {
            exact.push(EntityMatch { kind: EntityKind::Agent, id: a.agent_id.clone(), label: a.agent_name.clone() });
        } else if a.agent_id.starts_with(query) {
            prefix.push(EntityMatch { kind: EntityKind::Agent, id: a.agent_id.clone(), label: a.agent_name.clone() });
        }
    }

    // Check sessions
    let sessions = client.list_sessions().await?;
    for s in &sessions {
        if s.id == query {
            exact.push(EntityMatch { kind: EntityKind::Session, id: s.id.clone(), label: None });
        } else if s.id.starts_with(query) {
            prefix.push(EntityMatch { kind: EntityKind::Session, id: s.id.clone(), label: None });
        }
    }

    // Exact matches take priority; fall back to prefix matches
    if exact.is_empty() { Ok(prefix) } else { Ok(exact) }
}
```

**Ambiguity handling** (shared helper):

```rust
/// Print ambiguous matches and exit with code 1.
fn print_ambiguous(query: &str, command_name: &str, matches: &[EntityMatch]) -> ! {
    eprintln!("Ambiguous ID '{}' — matches multiple entities:\n", query);
    for m in matches {
        let kind_str = match m.kind {
            EntityKind::Pipeline => "pipeline",
            EntityKind::Agent => "agent",
            EntityKind::Session => "session",
        };
        let label = m.label.as_deref().unwrap_or("");
        eprintln!("  oj {} {} {}  {}", kind_str, command_name, m.id, label);
    }
    std::process::exit(1);
}
```

**No-match handling**: Print `"no entity found matching '{query}'"` and exit with code 1.

**Milestone:** `resolve_entity()` compiles and unit tests pass with mock data.

### Phase 2: Add `oj peek <id>` and `oj attach <id>` Commands

Add the two commands that don't need additional flags.

In `main.rs`, add to the `Commands` enum:

```rust
/// Peek at a pipeline/session (auto-detects entity type)
Peek {
    /// Entity ID (pipeline, agent, or session — prefix match supported)
    id: String,
},
/// Attach to a pipeline/session (auto-detects entity type)
Attach {
    /// Entity ID (pipeline, agent, or session — prefix match supported)
    id: String,
},
```

**Dispatch logic in `run()`** (both are query commands):

```rust
Commands::Peek { id } => {
    let client = DaemonClient::for_query()?;
    resolve::handle_peek(&client, &id, format).await?
}
Commands::Attach { id } => {
    let client = DaemonClient::for_query()?;
    resolve::handle_attach(&client, &id).await?
}
```

**`handle_peek` behavior by entity kind:**
- `Pipeline` → delegate to `pipeline::handle(PipelineCommand::Peek { id })` (which resolves session internally)
- `Agent` → delegate to `session::handle(SessionCommand::Peek { id })` (agent ID is the session ID)
- `Session` → delegate to `session::handle(SessionCommand::Peek { id })`

**`handle_attach` behavior by entity kind:**
- `Pipeline` → delegate to `pipeline::handle(PipelineCommand::Attach { id })`
- `Agent` → delegate to `session::attach(&id)` directly
- `Session` → delegate to `session::attach(&id)` directly

Note: `attach` and `peek` are not meaningful operations for agents as a separate concept from sessions — an agent's interactive session IS its session ID. So dispatching agents to the session command is correct.

**Milestone:** `oj peek abc` and `oj attach abc` work end-to-end, resolving across entity types.

### Phase 3: Add `oj logs <id>` with Flag Passthrough

Add the `logs` command with `-f/--follow` and `-n/--limit` flags.

```rust
/// View logs for a pipeline or agent (auto-detects entity type)
Logs {
    /// Entity ID (pipeline or agent — prefix match supported)
    id: String,
    /// Stream live activity (like tail -f)
    #[arg(long, short)]
    follow: bool,
    /// Number of recent lines to show (default: 50)
    #[arg(short = 'n', long, default_value = "50")]
    limit: usize,
    /// Show only a specific step's log (agent logs only)
    #[arg(long, short = 's')]
    step: Option<String>,
},
```

**Dispatch:**

```rust
Commands::Logs { id, follow, limit, step } => {
    let client = DaemonClient::for_query()?;
    resolve::handle_logs(&client, &id, follow, limit, step.as_deref(), format).await?
}
```

**`handle_logs` behavior by entity kind:**
- `Pipeline` → delegate to `pipeline::handle(PipelineCommand::Logs { id, follow, limit })`
- `Agent` → delegate to `agent::handle(AgentCommand::Logs { id, step, follow, limit })`
- `Session` → error: `"logs are not available for sessions — use 'oj peek {id}' instead"`

**Milestone:** `oj logs abc -f` works, resolving to pipeline or agent logs.

### Phase 4: Add `oj show <id>` with Flag Passthrough

Add the `show` command with `-v/--verbose` flag.

```rust
/// Show details of a pipeline (auto-detects entity type)
Show {
    /// Entity ID (pipeline — prefix match supported)
    id: String,
    /// Show full variable values without truncation
    #[arg(long, short = 'v')]
    verbose: bool,
},
```

**Dispatch:**

```rust
Commands::Show { id, verbose } => {
    let client = DaemonClient::for_query()?;
    resolve::handle_show(&client, &id, verbose, format).await?
}
```

**`handle_show` behavior by entity kind:**
- `Pipeline` → delegate to `pipeline::handle(PipelineCommand::Show { id, verbose })`
- `Agent` → Currently no `agent show` exists. For now, print agent summary info inline (agent_id, status, pipeline_id, step_name, files read/written, commands run). This is a simple formatted dump of `AgentSummary` fields.
- `Session` → Print session summary inline (id, pipeline_id, updated_at).

**Milestone:** `oj show abc -v` works, resolving to the appropriate display.

### Phase 5: Tests

Add tests in `crates/cli/src/commands/resolve_tests.rs` (following the project convention of `*_tests.rs` files).

**Unit tests for `resolve_entity`:**
- Exact match on pipeline ID returns single `Pipeline` match
- Exact match on agent ID returns single `Agent` match
- Exact match on session ID returns single `Session` match
- Prefix match with single hit returns that match
- Prefix match with multiple hits across entity types returns all
- Exact match takes priority over prefix matches
- No match returns empty vec

Tests will need to either:
- Mock `DaemonClient` (if a trait is available), or
- Test the resolution logic with extracted pure functions that take lists of summaries as input

The preferred approach is to extract the matching logic into a pure function:

```rust
pub fn resolve_from_lists(
    query: &str,
    pipelines: &[PipelineSummary],
    agents: &[AgentSummary],
    sessions: &[SessionSummary],
) -> Vec<EntityMatch> { ... }
```

This keeps the core logic testable without needing async client mocks.

**Milestone:** `cargo test --all` passes with new resolve tests.

## Key Implementation Details

### Command Naming in Clap

Clap derives subcommand names from variant names in lowercase. The new `Commands::Peek`, `Commands::Attach`, `Commands::Logs`, `Commands::Show` variants will generate `oj peek`, `oj attach`, `oj logs`, `oj show` automatically.

### Client Semantics

All four convenience commands are **query** commands (read-only), so they use `DaemonClient::for_query()`. This means the daemon must already be running — these commands will not auto-start it.

### Single-Match Fast Path

When resolution returns exactly one match, dispatch immediately without printing anything extra. The user experience should be identical to typing the fully-qualified command.

### Ambiguity Output Format

When multiple matches exist, output should be copy-paste friendly:

```
Ambiguous ID 'abc' — matches multiple entities:

  oj pipeline peek abc12345  my-pipeline
  oj agent peek abc67890
  oj session peek abcdef01
```

Each line is a complete, runnable command. Exit code 1.

### The `-o` / `--output` Global Flag

The output format flag is already global on the `Cli` struct, so it propagates to all subcommands automatically. The convenience commands should pass `format` through to the delegated handlers.

### Agent IDs as Session IDs

In the current architecture, an agent's `agent_id` doubles as its tmux session ID. This means `oj peek <agent_id>` and `oj attach <agent_id>` can delegate directly to session peek/attach without additional lookup.

## Verification Plan

1. **`make check`** — full CI suite (fmt, clippy, tests, build, audit, deny)
2. **Manual smoke tests:**
   - `oj peek <pipeline-prefix>` → shows tmux capture
   - `oj attach <session-id>` → attaches to tmux
   - `oj logs <pipeline-id> -f` → streams logs
   - `oj show <pipeline-id> -v` → shows detailed pipeline info
   - `oj peek <ambiguous-prefix>` → lists matches with exit code 1
   - `oj show <nonexistent>` → prints "no entity found" with exit code 1
3. **Unit tests** for `resolve_from_lists()` covering exact/prefix/ambiguous/empty cases
4. **Verify help text:** `oj --help` shows new commands, `oj peek --help` shows usage
