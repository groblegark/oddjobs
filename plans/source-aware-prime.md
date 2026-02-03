# Source-Aware Prime Commands

## Overview

Extend the agent `prime` field to support per-SessionStart-source commands. Currently, `prime` writes a single `prime.sh` script and registers one SessionStart hook with an empty matcher (fires on every session start). This feature adds an HCL block form that maps SessionStart source values (`startup`, `resume`, `clear`, `compact`) to distinct prime scripts, so agents can inject different context depending on how a session started.

## Project Structure

Files to create or modify:

```
crates/
├── runbook/src/
│   ├── agent.rs              # Extend PrimeDef enum, add PrimeConfig type
│   ├── agent_tests.rs        # Tests for new PrimeDef variant
│   └── parser.rs             # Validation for per-source commands
│   └── parser_tests/
│       └── prime.rs          # Parser tests for block form
├── engine/src/
│   ├── workspace.rs          # Multi-script generation, multi-hook injection
│   ├── workspace_tests.rs    # Tests for per-source hooks
│   └── spawn.rs              # Update call site for new API
```

No new files are needed.

## Dependencies

No new external dependencies. The existing `hcl-rs` serde deserialization handles HCL block forms automatically via `#[serde(untagged)]` enum variants.

## Implementation Phases

### Phase 1: Extend `PrimeDef` in `crates/runbook/src/agent.rs`

Add a third variant to the `PrimeDef` enum for per-source prime definitions.

**Changes to `PrimeDef`:**

```rust
/// Valid SessionStart source values for matcher filtering.
const VALID_PRIME_SOURCES: &[&str] = &["startup", "resume", "clear", "compact"];

#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum PrimeDef {
    Script(String),
    Commands(Vec<String>),
    PerSource(HashMap<String, PrimeDef>),
}
```

The `PerSource` variant holds a map from source name (`"startup"`, `"resume"`, `"clear"`, `"compact"`) to a `PrimeDef::Script` or `PrimeDef::Commands`. Nesting `PerSource` inside `PerSource` is invalid and should be rejected.

**Update `Deserialize` impl:**

Extend the `Helper` enum in the custom `Deserialize` implementation to include a `HashMap<String, PrimeDef>` variant. Since serde tries variants in order, `Script` (string) and `Commands` (array) are attempted first; the map variant matches HCL block bodies.

```rust
impl<'de> Deserialize<'de> for PrimeDef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Helper {
            Script(String),
            Commands(Vec<String>),
            PerSource(HashMap<String, PrimeDef>),
        }
        match Helper::deserialize(deserializer)? {
            Helper::Script(s) => Ok(PrimeDef::Script(s)),
            Helper::Commands(v) => Ok(PrimeDef::Commands(v)),
            Helper::PerSource(map) => {
                // Validate: no nested PerSource, only valid source keys
                for (key, val) in &map {
                    if matches!(val, PrimeDef::PerSource(_)) {
                        return Err(serde::de::Error::custom(
                            "nested per-source prime is not allowed",
                        ));
                    }
                }
                Ok(PrimeDef::PerSource(map))
            }
        }
    }
}
```

**Update `render()` method:**

The `render()` method on `PrimeDef` is only valid for `Script` and `Commands`. For `PerSource`, callers must iterate the map and render each inner `PrimeDef` individually. Add a helper:

```rust
impl PrimeDef {
    /// Render the prime script content with variable interpolation.
    /// Panics if called on PerSource — use `render_per_source()` instead.
    pub fn render(&self, vars: &HashMap<String, String>) -> String {
        match self {
            PrimeDef::Script(s) => crate::template::interpolate(s, vars),
            PrimeDef::Commands(cmds) => cmds
                .iter()
                .map(|cmd| crate::template::interpolate(cmd, vars))
                .collect::<Vec<_>>()
                .join("\n"),
            PrimeDef::PerSource(_) => {
                panic!("render() not valid for PerSource; use render_per_source()")
            }
        }
    }

    /// For PerSource, iterate entries and render each inner PrimeDef.
    /// For Script/Commands, returns a single-entry map with empty string key (all sources).
    pub fn render_per_source(
        &self,
        vars: &HashMap<String, String>,
    ) -> HashMap<String, String> {
        match self {
            PrimeDef::PerSource(map) => map
                .iter()
                .map(|(source, def)| (source.clone(), def.render(vars)))
                .collect(),
            other => {
                let mut m = HashMap::new();
                m.insert(String::new(), other.render(vars));
                m
            }
        }
    }
}
```

**Milestone:** `PrimeDef` compiles with all three variants. Existing `Script`/`Commands` paths still work. Unit tests pass.

### Phase 2: Parser validation in `crates/runbook/src/parser.rs`

Extend the prime validation block (lines 200-204) to handle `PerSource`.

```rust
if let Some(ref prime) = agent.prime {
    match prime {
        PrimeDef::Commands(cmds) => {
            for (i, cmd) in cmds.iter().enumerate() {
                validate_shell_command(
                    cmd,
                    &format!("agent.{}.prime[{}]", name, i),
                )?;
            }
        }
        PrimeDef::PerSource(map) => {
            for (source, def) in map {
                // Validate source key
                if !VALID_PRIME_SOURCES.contains(&source.as_str()) {
                    return Err(ParseError::InvalidFormat {
                        location: format!("agent.{}.prime", name),
                        message: format!(
                            "unknown prime source '{}'; valid sources: {}",
                            source,
                            VALID_PRIME_SOURCES.join(", ")
                        ),
                    });
                }
                // Validate inner commands
                if let PrimeDef::Commands(cmds) = def {
                    for (i, cmd) in cmds.iter().enumerate() {
                        validate_shell_command(
                            cmd,
                            &format!("agent.{}.prime.{}[{}]", name, source, i),
                        )?;
                    }
                }
            }
        }
        PrimeDef::Script(_) => {} // Script form: no per-command validation
    }
}
```

Import `VALID_PRIME_SOURCES` from `oj_runbook::agent` (or `oj_runbook`).

**Milestone:** Invalid source keys and malformed inner commands are caught at parse time. Existing prime forms still validate correctly.

### Phase 3: Multi-script generation in `crates/engine/src/workspace.rs`

#### 3a: Update `prepare_agent_prime()`

Change the return type from `PathBuf` to `HashMap<String, PathBuf>` — a map from source matcher to script path. For `Script`/`Commands`, the map has one entry with key `""` (empty matcher = all sources). For `PerSource`, each entry gets its own script file.

```rust
/// Write agent prime script(s) to the state directory.
///
/// Returns a map of SessionStart matcher -> script path.
/// - For Script/Commands: single entry with empty matcher ("" = all sources)
/// - For PerSource: one entry per source (e.g., "startup" -> prime-startup.sh)
pub fn prepare_agent_prime(
    agent_id: &str,
    prime: &PrimeDef,
    vars: &HashMap<String, String>,
) -> io::Result<HashMap<String, PathBuf>> {
    let agent_dir = agent_state_dir(agent_id)?;
    let rendered = prime.render_per_source(vars);

    let mut paths = HashMap::new();
    for (source, content) in &rendered {
        let filename = if source.is_empty() {
            "prime.sh".to_string()
        } else {
            format!("prime-{}.sh", source)
        };
        let path = agent_dir.join(&filename);
        let script = format!(
            "#!/usr/bin/env bash\nset -euo pipefail\n{}\n",
            content
        );
        fs::write(&path, &script)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
        }

        paths.insert(source.clone(), path);
    }

    Ok(paths)
}
```

Extract the state dir lookup into a small helper (`agent_state_dir`) to reduce duplication with `agent_settings_path`.

#### 3b: Update `prepare_agent_settings()` and `inject_hooks()`

Change `prime_path: Option<&Path>` to `prime_paths: &HashMap<String, PathBuf>` (or `Option<&HashMap<String, PathBuf>>`). Generate one SessionStart hook entry per map entry, each with the appropriate matcher.

```rust
pub fn prepare_agent_settings(
    agent_id: &str,
    workspace_path: &Path,
    prime_paths: &HashMap<String, PathBuf>,
) -> io::Result<PathBuf> { ... }

fn inject_hooks(
    settings: &mut Value,
    agent_id: &str,
    prime_paths: &HashMap<String, PathBuf>,
) {
    // ... existing Stop, Notification, PreToolUse hooks unchanged ...

    // Inject SessionStart hooks — one entry per prime source
    if !prime_paths.is_empty() {
        let session_start_entries: Vec<Value> = prime_paths
            .iter()
            .map(|(matcher, path)| {
                json!({
                    "matcher": matcher,
                    "hooks": [{
                        "type": "command",
                        "command": format!("bash {}", path.display())
                    }]
                })
            })
            .collect();
        hooks_obj.insert("SessionStart".to_string(), json!(session_start_entries));
    }
}
```

**Milestone:** Per-source prime scripts are written as separate files. Settings JSON contains multiple SessionStart hook entries with correct matchers.

### Phase 4: Update `spawn.rs` call site

Update `crates/engine/src/spawn.rs` (lines 96-113) to use the new return type.

```rust
// Write prime script(s) if agent has prime config
let prime_paths = if let Some(ref prime) = agent_def.prime {
    crate::workspace::prepare_agent_prime(&agent_id, prime, &prompt_vars)
        .map_err(|e| {
            tracing::error!(error = %e, "agent prime preparation failed");
            RuntimeError::Execute(ExecuteError::Shell(e.to_string()))
        })?
} else {
    HashMap::new()
};

// Prepare settings file with hooks in OJ state directory
let settings_path =
    crate::workspace::prepare_agent_settings(&agent_id, workspace_path, &prime_paths)
        .map_err(|e| { ... })?;
```

**Milestone:** Full pipeline works end-to-end. Existing agents with string/array prime are unaffected.

### Phase 5: Tests

#### 5a: `crates/runbook/src/agent_tests.rs`

- `prime_deserialize_per_source_form()` — deserialize a `PerSource` map
- `prime_render_per_source()` — renders each source independently
- `prime_render_script_as_per_source()` — `render_per_source` on Script returns single empty-key entry
- `prime_per_source_rejects_nested()` — nested PerSource fails deserialization

#### 5b: `crates/runbook/src/parser_tests/prime.rs`

- `parse_hcl_agent_with_per_source_prime()` — HCL block form parses correctly
- `parse_hcl_agent_with_per_source_prime_string_values()` — string values inside block
- `error_per_source_prime_invalid_source()` — unknown source key rejected
- `error_per_source_prime_invalid_shell()` — invalid shell in per-source commands rejected
- `parse_toml_agent_with_per_source_prime()` — TOML table form (if supported)

#### 5c: `crates/engine/src/workspace_tests.rs`

- `prepare_agent_prime_per_source_writes_multiple_scripts()` — separate files per source
- `prepare_agent_settings_per_source_injects_multiple_session_start_hooks()` — multiple SessionStart entries with matchers
- `prepare_agent_settings_empty_prime_paths_no_session_start()` — empty map produces no hook
- `prepare_agent_prime_backward_compat_single_script()` — Script/Commands still produce `prime.sh` with empty matcher

**Milestone:** All tests pass. `make check` succeeds.

## Key Implementation Details

### Serde Untagged Enum Ordering

The `Helper` enum in `PrimeDef::deserialize` **must** try `Script` and `Commands` before `PerSource`. If `PerSource(HashMap<String, PrimeDef>)` were listed first, a string value would fail to match and produce a confusing error. The current ordering (String → Vec → Map) matches serde's untagged try-in-order semantics.

### HCL Block vs Attribute

In HCL, `prime { startup = [...] }` is a block, while `prime = [...]` is an attribute. The `hcl-rs` crate's serde layer handles this transparently — blocks with labeled bodies become maps, attributes become their literal types. No custom HCL parsing is needed.

### File Naming Convention

Per-source scripts use the pattern `prime-{source}.sh` (e.g., `prime-startup.sh`, `prime-resume.sh`). The default (all-sources) script remains `prime.sh`. This avoids collisions and makes debugging easy — `ls $OJ_STATE_DIR/agents/{id}/` shows which sources have scripts.

### Backward Compatibility

- `prime = "script"` and `prime = ["cmd1", "cmd2"]` continue to work identically
- The `render()` method still works for `Script`/`Commands` (panics on `PerSource` to catch misuse)
- `render_per_source()` works for all variants, returning a single-entry map for the legacy forms
- `prepare_agent_settings()` signature changes from `Option<&Path>` to `&HashMap<String, PathBuf>`, but this is an internal API (only called from `spawn.rs`)

### Omitted Sources Get No Hook

If a `prime {}` block only specifies `startup` and `resume`, then `clear` and `compact` session starts will have **no** SessionStart hook — Claude starts with no injected context for those sources. This is intentional and matches the specification.

## Verification Plan

1. **Unit tests** (`cargo test -p oj-runbook`):
   - PrimeDef deserialization for all three variants
   - PrimeDef rendering for all three variants
   - Rejection of invalid source keys
   - Rejection of nested PerSource
   - Shell validation within per-source commands

2. **Engine tests** (`cargo test -p oj-engine`):
   - Multi-script file generation (correct filenames, content, permissions)
   - Settings JSON with multiple SessionStart entries and correct matchers
   - Backward compatibility for single-script prime

3. **Parser integration tests** (`cargo test -p oj-runbook`):
   - HCL block form parsing end-to-end
   - TOML table form parsing (if applicable)
   - Mixed agent definitions (some with block prime, some with string prime)

4. **Full check** (`make check`):
   - `cargo fmt --all -- --check`
   - `cargo clippy --all-targets --all-features -- -D warnings`
   - `quench check`
   - `cargo test --all`
   - `cargo build --all`
   - `cargo audit`
   - `cargo deny check licenses bans sources`
