# Agent List Command

## Overview

Add `oj agent list` CLI command that lists agents across all pipelines. Agents are derived from pipeline step history (no separate storage) — the command iterates pipelines, extracts agent summaries using the existing `compute_agent_summaries` pattern, and displays them in a columnar text format. Supports filtering by pipeline ID, status, and result limiting.

## Project Structure

Files to create or modify:

```
crates/daemon/src/protocol.rs       # Add ListAgents query variant + Agents response variant
crates/daemon/src/listener/query.rs # Add ListAgents handler
crates/cli/src/client.rs            # Add list_agents() client method
crates/cli/src/commands/agent.rs    # Add AgentCommand::List variant + handler
```

No new files needed. All changes extend existing modules.

## Dependencies

No new external dependencies. Uses existing `clap`, `serde`, `serde_json`.

## Implementation Phases

### Phase 1: Protocol — Add `ListAgents` Query and Response

**Files:** `crates/daemon/src/protocol.rs`

1. Add `ListAgents` variant to the `Query` enum with optional filters:

```rust
/// List agents across all pipelines
ListAgents {
    /// Filter by pipeline ID prefix
    #[serde(default)]
    pipeline_id: Option<String>,
    /// Filter by status (e.g. "running", "completed", "failed", "waiting")
    #[serde(default)]
    status: Option<String>,
},
```

2. Add `Agents` variant to the `Response` enum:

```rust
/// List of agents
Agents { agents: Vec<AgentSummary> },
```

3. Add `pipeline_id` field to `AgentSummary` so agents can be displayed alongside the pipeline they belong to:

```rust
pub struct AgentSummary {
    pub pipeline_id: String,    // NEW
    pub step_name: String,
    pub agent_id: String,
    pub status: String,
    pub files_read: usize,
    pub files_written: usize,
    pub commands_run: usize,
    pub exit_reason: Option<String>,
}
```

**Milestone:** `cargo check -p oj-daemon` passes. Existing `GetPipeline` handler still compiles (it already populates `AgentSummary` — just needs to set the new `pipeline_id` field).

### Phase 2: Query Handler — Implement `ListAgents`

**Files:** `crates/daemon/src/listener/query.rs`

1. Update the `compute_agent_summaries` function signature to accept a `pipeline_id: &str` parameter, and set it on each returned `AgentSummary`.

2. Update the existing `Query::GetPipeline` call site to pass `p.id` to `compute_agent_summaries`.

3. Add the `Query::ListAgents` match arm in `handle_query()`:

```rust
Query::ListAgents { pipeline_id, status } => {
    let mut agents: Vec<AgentSummary> = Vec::new();

    for p in state.pipelines.values() {
        // Filter by pipeline_id prefix if specified
        if let Some(ref prefix) = pipeline_id {
            if !p.id.starts_with(prefix.as_str()) {
                continue;
            }
        }

        // Build StepRecordDetail list (same as GetPipeline)
        let steps: Vec<StepRecordDetail> = p.step_history.iter().map(|r| {
            StepRecordDetail { /* same mapping as GetPipeline */ }
        }).collect();

        let mut summaries = compute_agent_summaries(&p.id, &steps, logs_path);

        // Filter by status if specified
        if let Some(ref s) = status {
            summaries.retain(|a| a.status == *s);
        }

        agents.extend(summaries);
    }

    Response::Agents { agents }
}
```

4. Update the import list at the top of `query.rs` to include `AgentSummary` (it's already imported — just confirm).

**Milestone:** `cargo check -p oj-daemon` passes. Can manually test via `oj daemon` + raw socket query.

### Phase 3: Client Method — `list_agents()`

**Files:** `crates/cli/src/client.rs`

Add `list_agents()` following the `list_pipelines()` pattern:

```rust
/// Query for agents across all pipelines
pub async fn list_agents(
    &self,
    pipeline_id: Option<&str>,
    status: Option<&str>,
) -> Result<Vec<oj_daemon::AgentSummary>, ClientError> {
    let query = Request::Query {
        query: Query::ListAgents {
            pipeline_id: pipeline_id.map(|s| s.to_string()),
            status: status.map(|s| s.to_string()),
        },
    };
    match self.send(&query).await? {
        Response::Agents { agents } => Ok(agents),
        Response::Error { message } => Err(ClientError::Rejected(message)),
        _ => Err(ClientError::UnexpectedResponse),
    }
}
```

**Milestone:** `cargo check -p oj-cli` passes.

### Phase 4: CLI Command — `AgentCommand::List`

**Files:** `crates/cli/src/commands/agent.rs`

1. Add `List` variant to `AgentCommand`:

```rust
/// List agents across all pipelines
List {
    /// Filter by pipeline ID (or prefix)
    #[arg(long)]
    pipeline: Option<String>,

    /// Filter by status (e.g. "running", "completed", "failed", "waiting")
    #[arg(long)]
    status: Option<String>,

    /// Maximum number of agents to show (default: 20)
    #[arg(short = 'n', long, default_value = "20")]
    limit: usize,

    /// Show all agents (no limit)
    #[arg(long, conflicts_with = "limit")]
    no_limit: bool,
},
```

2. Add match arm in `handle()`:

```rust
AgentCommand::List { pipeline, status, limit, no_limit } => {
    let agents = client.list_agents(
        pipeline.as_deref(),
        status.as_deref(),
    ).await?;

    let total = agents.len();
    let display_limit = if no_limit { total } else { limit };
    let agents: Vec<_> = agents.into_iter().take(display_limit).collect();
    let remaining = total.saturating_sub(display_limit);

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&agents)?);
        }
        OutputFormat::Text => {
            if agents.is_empty() {
                println!("No agents found");
            } else {
                println!(
                    "{:<12} {:<12} {:<16} {:<10} {:>5} {:>5} {:>4}",
                    "AGENT_ID", "PIPELINE", "STEP", "STATUS",
                    "READ", "WRITE", "CMDS"
                );
                for a in &agents {
                    println!(
                        "{:<12} {:<12} {:<16} {:<10} {:>5} {:>5} {:>4}",
                        &a.agent_id[..a.agent_id.len().min(12)],
                        &a.pipeline_id[..a.pipeline_id.len().min(12)],
                        &a.step_name[..a.step_name.len().min(16)],
                        &a.status[..a.status.len().min(10)],
                        a.files_read,
                        a.files_written,
                        a.commands_run,
                    );
                }
            }
            if remaining > 0 {
                println!(
                    "\n... {} more not shown. Use --no-limit or -n N to see more.",
                    remaining
                );
            }
        }
    }
}
```

3. Add `use crate::output::OutputFormat;` if not already imported (it is).

**Milestone:** `oj agent list` works end-to-end. `oj agent list --status running` filters correctly. `oj agent list --pipeline <prefix>` filters by pipeline. JSON output works with `--format json`.

### Phase 5: Verification and Cleanup

1. Run `make check` (fmt, clippy, tests, build, audit, deny).
2. Verify `oj agent list` with no agents returns "No agents found".
3. Verify `--limit` / `--no-limit` truncation message.
4. Verify `--format json` returns valid JSON array.

## Key Implementation Details

### Agent derivation from pipeline state
Agents have no separate storage. They are derived by iterating `state.pipelines` → `step_history` → filtering steps with `agent_id.is_some()`, then enriching via `compute_agent_summaries` which parses agent log files for file/command counts.

### `pipeline_id` field addition
The existing `AgentSummary` struct lacks a `pipeline_id` field. Adding it is necessary so the list view can show which pipeline an agent belongs to. The `GetPipeline` response already wraps agents inside a `PipelineDetail` (so `pipeline_id` is implicit there), but the flat `Agents` response needs it explicit. This is a minor struct change — update the one existing call site in `GetPipeline` to populate it.

### Sorting
Agents should be returned in a useful order. Sort by: running agents first, then by pipeline recency (most recent first). This mirrors the `pipeline list` sorting pattern. The sorting happens server-side in the query handler.

### Column widths
Column headers match the instruction: `AGENT_ID`, `PIPELINE`, `STEP`, `STATUS`, `FILES_READ`, `FILES_WRITTEN`, `COMMANDS`. Shortened to `READ`, `WRITE`, `CMDS` in the header to keep lines under ~80 chars. Full names used for JSON keys.

### Filtering is server-side
Both `pipeline_id` and `status` filters are passed to the daemon query, avoiding unnecessary log file parsing for filtered-out pipelines. This is important because `compute_agent_summaries` does filesystem I/O (reading agent log files).

## Verification Plan

1. **Unit tests:** Not strictly needed — the query handler is a thin integration of existing functions. If desired, add a test in `crates/daemon/src/listener/` that constructs a `MaterializedState` with pipelines containing step history and verifies the `ListAgents` response.

2. **Manual smoke test:**
   - Start daemon: `oj daemon`
   - Run a pipeline that spawns agents
   - `oj agent list` → shows agents with correct columns
   - `oj agent list --status running` → filters to running only
   - `oj agent list --pipeline <id-prefix>` → filters by pipeline
   - `oj agent list -n 1` → shows 1 agent + "... N more" message
   - `oj agent list --no-limit` → shows all
   - `oj agent list --format json` → valid JSON output

3. **`make check`:** Full suite passes (fmt, clippy, test, build, audit, deny).
