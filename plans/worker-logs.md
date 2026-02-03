# Worker Logs

## Overview

Add `oj worker logs <name>` command that displays worker activity logs with support for follow mode and line limits. This involves:
1. A `worker_log_path` helper in `crates/engine/src/log_paths.rs`
2. A `WorkerLogger` that writes timestamped entries during worker lifecycle events
3. Instrumentation in the worker handler to log start, stop, dispatch, error, and idle/wake events
4. A `GetWorkerLogs` query through the daemon IPC protocol
5. A CLI `Logs` subcommand wired through the client

## Project Structure

```
crates/
├── engine/src/
│   ├── log_paths.rs            # + worker_log_path()
│   ├── log_paths_tests.rs      # + test for worker_log_path
│   ├── worker_logger.rs        # NEW — WorkerLogger (append-only, like PipelineLogger)
│   ├── runtime/handlers/
│   │   └── worker.rs           # + log calls at lifecycle points
│   └── lib.rs                  # + pub mod worker_logger
├── daemon/src/
│   ├── protocol.rs             # + GetWorkerLogs query, WorkerLogs response
│   └── listener/query.rs       # + handler for GetWorkerLogs
├── cli/src/
│   ├── commands/worker.rs      # + Logs subcommand
│   └── client.rs               # + get_worker_logs()
```

## Dependencies

No new external dependencies. Uses the same `std::fs`, `std::io`, `std::time` primitives as `PipelineLogger`.

## Implementation Phases

### Phase 1: Log path helper

Add `worker_log_path` to `crates/engine/src/log_paths.rs`:

```rust
/// Build the path to a worker log file.
///
/// Structure: `{logs_dir}/worker/{worker_name}.log`
pub fn worker_log_path(logs_dir: &Path, worker_name: &str) -> PathBuf {
    logs_dir.join("worker").join(format!("{}.log", worker_name))
}
```

Add a unit test in `log_paths_tests.rs` following the existing pattern.

Update the module doc comment to mention worker logs alongside agent logs.

### Phase 2: WorkerLogger

Create `crates/engine/src/worker_logger.rs` modeled on `PipelineLogger` (`crates/engine/src/pipeline_logger.rs`):

```rust
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::log_paths;

/// Append-only logger for per-worker activity logs.
///
/// Writes human-readable timestamped lines to:
///   `<log_dir>/worker/<worker_name>.log`
///
/// Format: `2026-01-30T08:14:09Z [worker] message`
pub struct WorkerLogger {
    log_dir: PathBuf,
}

impl WorkerLogger {
    pub fn new(log_dir: PathBuf) -> Self {
        Self { log_dir }
    }

    pub fn append(&self, worker_name: &str, message: &str) {
        let path = log_paths::worker_log_path(&self.log_dir, worker_name);
        if let Err(e) = self.write_line(&path, message) {
            tracing::warn!(
                worker_name,
                error = %e,
                "failed to write worker log"
            );
        }
    }

    fn write_line(&self, path: &Path, message: &str) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        let ts = format_utc_now();  // reuse from pipeline_logger or extract to shared util
        writeln!(file, "{} [worker] {}", ts, message)?;
        Ok(())
    }
}
```

**Key decision**: Reuse the `format_utc_now` function. Either:
- Extract it from `pipeline_logger.rs` into a shared internal helper (e.g. `crate::time_fmt`), or
- Duplicate it in `worker_logger.rs` (simpler, avoids refactor)

Preferred: extract to a small shared module `crates/engine/src/time_fmt.rs` with `pub(crate) fn format_utc_now()` and `days_to_civil()`, then use from both `PipelineLogger` and `WorkerLogger`. This avoids duplication without expanding the public API.

Export `WorkerLogger` from `crates/engine/src/lib.rs`.

### Phase 3: Instrument worker handler

Add a `WorkerLogger` field to the `Runtime` struct (alongside the existing `PipelineLogger`). Initialize it with the same `log_dir`.

In `crates/engine/src/runtime/handlers/worker.rs`, add log calls:

| Event | Location | Log message |
|---|---|---|
| **start** | `handle_worker_started`, after state stored | `started (queue={queue_name}, concurrency={N})` |
| **stop** | `handle_worker_stopped` | `stopped` |
| **dispatch** | `handle_worker_poll_complete`, after pipeline created | `dispatched item {item_id} → pipeline {pipeline_id}` |
| **idle** | `handle_worker_poll_complete`, when `items.is_empty()` or `available == 0` | `idle (active={N}/{concurrency})` |
| **wake** | `handle_worker_wake`, at entry | `wake` |
| **error** | `handle_worker_poll_complete`, on take command failure | `error: take command failed for item {item_id}` |
| **pipeline complete** | `check_worker_pipeline_complete`, after pipeline removed | `pipeline {pipeline_id} completed (step={terminal_step}), active={N}/{concurrency}` |

Example instrumentation in `handle_worker_started`:

```rust
// After storing worker state:
self.worker_logger.append(
    worker_name,
    &format!("started (queue={}, concurrency={})", worker_def.source.queue, worker_def.concurrency),
);
```

### Phase 4: IPC protocol additions

In `crates/daemon/src/protocol.rs`:

**Add query variant:**
```rust
// In enum Query:
GetWorkerLogs {
    name: String,
    #[serde(default)]
    namespace: String,
    lines: usize,
},
```

**Add response variant:**
```rust
// In enum Response:
WorkerLogs {
    log_path: PathBuf,
    content: String,
},
```

### Phase 5: Daemon query handler

In `crates/daemon/src/listener/query.rs`, add a match arm for `GetWorkerLogs`:

```rust
Query::GetWorkerLogs { name, namespace, lines } => {
    use oj_engine::log_paths::worker_log_path;

    // Scope worker name by namespace (same as worker state key)
    let scoped_name = if namespace.is_empty() {
        name.clone()
    } else {
        format!("{}/{}", namespace, name)
    };

    let log_path = worker_log_path(logs_path, &scoped_name);
    let content = read_log_file(&log_path, lines);
    Response::WorkerLogs { log_path, content }
}
```

**Key decision on log file naming**: Worker names are unique within a namespace. The file path is `{logs_dir}/worker/{namespace}/{worker_name}.log` if namespaced, or `{logs_dir}/worker/{worker_name}.log` if not. Since `worker_log_path` takes a single string, the caller constructs the scoped name. This matches how worker state keys work elsewhere.

Alternatively, keep it simple: the worker handler already knows the namespace, so pass `{namespace}/{worker_name}` as the worker_name to `WorkerLogger.append()`. The query handler does the same scoping. This avoids changing the `worker_log_path` signature.

### Phase 6: CLI command and client method

**Client method** in `crates/cli/src/client.rs`:

```rust
/// Get worker logs
pub async fn get_worker_logs(
    &self,
    name: &str,
    namespace: &str,
    lines: usize,
) -> Result<(PathBuf, String), ClientError> {
    let request = Request::Query {
        query: Query::GetWorkerLogs {
            name: name.to_string(),
            namespace: namespace.to_string(),
            lines,
        },
    };
    match self.send(&request).await? {
        Response::WorkerLogs { log_path, content } => Ok((log_path, content)),
        other => Self::reject(other),
    }
}
```

**CLI subcommand** in `crates/cli/src/commands/worker.rs`:

Add to `WorkerCommand` enum:
```rust
/// View worker activity log
Logs {
    /// Worker name
    name: String,
    /// Stream live activity (like tail -f)
    #[arg(long, short)]
    follow: bool,
    /// Number of recent lines to show (default: 50)
    #[arg(short = 'n', long, default_value = "50")]
    limit: usize,
    /// Project namespace override
    #[arg(long = "project")]
    project: Option<String>,
},
```

Add to the `handle` match:
```rust
WorkerCommand::Logs { name, follow, limit, project } => {
    let effective_namespace = project
        .or_else(|| std::env::var("OJ_NAMESPACE").ok())
        .unwrap_or_else(|| namespace.to_string());

    let (log_path, content) = client
        .get_worker_logs(&name, &effective_namespace, limit)
        .await?;
    display_log(&log_path, &content, follow, format, "worker", &name).await?;
}
```

This reuses the existing `display_log` helper from `crates/cli/src/output.rs` which already handles `--follow` via `tail_file`.

## Key Implementation Details

### Log format
All entries follow the pipeline log convention: `YYYY-MM-DDTHH:MM:SSZ [worker] message`. Using `[worker]` as the fixed step label keeps the format consistent with pipeline logs (`[step_name]`) while making worker entries easily grep-able.

### Namespace scoping for log files
Worker log files are scoped by namespace to avoid collisions between projects. The log path becomes `{logs_dir}/worker/{namespace}/{worker_name}.log` (by passing `{namespace}/{worker_name}` to `worker_log_path`). For the empty namespace case, it becomes `{logs_dir}/worker/{worker_name}.log`.

### Error resilience
Following `PipelineLogger`'s pattern, all write failures are logged via `tracing::warn` but never propagate — logging must not break the engine.

### Shared timestamp formatting
Extract `format_utc_now` and `days_to_civil` from `pipeline_logger.rs` into `crates/engine/src/time_fmt.rs` as `pub(crate)` functions, then use from both loggers. Update `pipeline_logger.rs` to call `crate::time_fmt::format_utc_now()`.

## Verification Plan

1. **Phase 1**: `cargo test -p oj-engine` — new `worker_log_path` test passes
2. **Phase 2**: `cargo test -p oj-engine` — `WorkerLogger` unit test writes and reads back a log line
3. **Phase 3**: `cargo test -p oj-engine` — existing worker handler tests still pass; verify log file created in test log dir
4. **Phase 4**: `cargo test -p oj-daemon` — protocol round-trip test for `GetWorkerLogs`/`WorkerLogs`
5. **Phase 5**: `cargo test -p oj-daemon` — query handler test for `GetWorkerLogs`
6. **Phase 6**: `cargo build --all` — full build passes; `cargo clippy --all-targets --all-features -- -D warnings`; `make check`
7. **Manual**: Start a worker, verify log file appears at `~/.local/state/oj/logs/worker/<name>.log`, run `oj worker logs <name>` and `oj worker logs <name> -f`
