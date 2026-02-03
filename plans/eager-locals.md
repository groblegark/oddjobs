# Eager Locals

## Overview

Evaluate shell expressions (`$(...)`) in pipeline locals eagerly at pipeline creation time, then remove the `interpolate_shell_trusted` concept entirely. Currently, locals containing `$(cmd)` are stored as literal strings and passed through unescaped at step execution via trusted prefixes. This is a security problem: user-provided `var.*` values flow through locals unescaped (e.g. `local.title = "fix: ${var.bug.title}"` where `bug.title` contains double quotes breaks shell commands). After this change, all `$(...)` expressions in locals are evaluated to plain data at creation time, and all shell interpolation uses uniform escaping with no trusted/untrusted distinction.

## Project Structure

Files to modify:

```
crates/engine/src/runtime/handlers/pipeline_create.rs  # Eager evaluation of $() in locals
crates/engine/src/runtime/pipeline.rs                   # Switch to interpolate_shell()
crates/runbook/src/template.rs                          # Remove interpolate_shell_trusted
crates/runbook/src/template_tests.rs                    # Update tests
crates/runbook/src/lib.rs                               # Remove public export
crates/engine/src/runtime_tests/steps.rs                # Update integration test
```

## Dependencies

No new dependencies. `tokio::process::Command` is already available via the `tokio` dependency in `oj-engine`.

## Implementation Phases

### Phase 1: Eagerly evaluate `$(...)` in locals at pipeline creation

**File:** `crates/engine/src/runtime/handlers/pipeline_create.rs` (lines 131–147)

After each local is interpolated with `interpolate()`, check if the resulting value contains `$(`. If so, run it through `bash -c` and store the captured stdout (trimmed) as the local's value. Use `invoke.dir` (or cwd fallback) as the working directory.

```rust
// After interpolating each local with interpolate():
for (key, template) in &pipeline_def.locals {
    let value = oj_runbook::interpolate(template, &lookup);

    // Eagerly evaluate shell expressions — $(cmd) becomes plain data
    let value = if value.contains("$(") {
        let cwd = vars
            .get("invoke.dir")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        let output = tokio::process::Command::new("bash")
            .arg("-c")
            .arg(&format!("printf '%s' {}", value))
            .current_dir(&cwd)
            .output()
            .await
            .map_err(|e| RuntimeError::ShellError(format!(
                "failed to evaluate local.{}: {}", key, e
            )))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(RuntimeError::ShellError(format!(
                "local.{} evaluation failed: {}", key, stderr.trim()
            )));
        }
        String::from_utf8_lossy(&output.stdout).to_string()
    } else {
        value
    };

    lookup.insert(format!("local.{}", key), value.clone());
    vars.insert(format!("local.{}", key), value);
}
```

**Notes:**
- The function `create_and_start_pipeline` must become `async` (it already is).
- Use `printf '%s' <value>` rather than `echo` to evaluate `$(...)` within the value while preserving the surrounding literal text. The value itself acts as the shell expression — e.g. value `$(git rev-parse --show-toplevel)` becomes `printf '%s' $(git rev-parse --show-toplevel)`.
- Errors in shell evaluation should fail the pipeline creation (return `RuntimeError`).
- Add a `ShellError` variant to `RuntimeError` if one doesn't exist, or reuse an existing variant.

**Verification:** `cargo test -p oj-engine` — the existing `locals_preserve_shell_syntax_in_stored_value` test will need updating (Phase 5).

### Phase 2: Switch shell step interpolation from `interpolate_shell_trusted` to `interpolate_shell`

**File:** `crates/engine/src/runtime/pipeline.rs` (lines 74–78)

Replace:
```rust
let command = oj_runbook::interpolate_shell_trusted(
    cmd,
    &vars,
    &["local.", "workspace.", "invoke."],
);
```

With:
```rust
let command = oj_runbook::interpolate_shell(cmd, &vars);
```

All values — including `local.*`, `workspace.*`, and `invoke.*` — are now plain data (no shell syntax to preserve), so uniform escaping is correct and safe.

**Verification:** `cargo test -p oj-engine` passes.

### Phase 3: Remove `interpolate_shell_trusted` from template module

**File:** `crates/runbook/src/template.rs`

1. Delete the `interpolate_shell_trusted` function (lines 70–82).
2. Remove the `trusted_prefixes` parameter from `interpolate_inner` — simplify the signature to `(template, vars, shell_escape)`.
3. Remove the `is_trusted` branch from the match arm in `interpolate_inner` (lines 104–110), leaving just `escape_for_shell(val)` for the `shell_escape` case.

**File:** `crates/runbook/src/lib.rs`

Remove `interpolate_shell_trusted` from the `pub use template::` line (line 37).

**Verification:** `cargo test -p oj-runbook` and `cargo build --all` pass.

### Phase 4: Update tests

**File:** `crates/runbook/src/template_tests.rs`

Delete these tests that exercise trusted prefixes:
- `interpolate_shell_trusted_skips_escaping_for_trusted_prefix` (line 150)
- `interpolate_shell_trusted_still_escapes_untrusted` (line 164)
- `interpolate_shell_trusted_mixed_trusted_and_untrusted` (line 176)
- `interpolate_shell_trusted_empty_prefixes_escapes_all` (line 201)

No replacement tests needed — `interpolate_shell` tests already cover the uniform escaping behavior.

**File:** `crates/engine/src/runtime_tests/steps.rs`

Update `locals_preserve_shell_syntax_in_stored_value` (line 912) to verify that locals with `$(...)` are eagerly evaluated. The test runbook uses `$(echo /some/repo)` — after eager evaluation, `local.repo` should contain `/some/repo` (the output), not the literal `$(echo /some/repo)`.

```rust
// After eager evaluation, $(echo /some/repo) should be resolved
assert_eq!(
    pipeline.vars.get("local.repo").map(String::as_str),
    Some("/some/repo"),
    "Shell command substitution should be eagerly evaluated in locals"
);
```

**Note:** This test uses `FakeAdapters`. Verify that the test harness supports `tokio::process::Command` execution (it should, since the executor tests already run shell commands). If the test environment can't run bash, the test may need a `#[cfg]` guard or the test runbook may need adjustment.

**Verification:** `cargo test --all` passes.

### Phase 5: Verify with `make check`

Run the full verification suite:
- `cargo fmt --all -- --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `quench check`
- `cargo test --all`
- `cargo build --all`

Ensure no dead code warnings from removing the trusted prefix concept.

## Key Implementation Details

### Why `printf '%s'` instead of `echo`

The interpolated value like `$(git -C /path rev-parse --show-toplevel)` needs to be evaluated as a shell expression. Wrapping it with `printf '%s'` ensures:
1. The `$(...)` is evaluated by bash
2. Surrounding literal text is preserved
3. No trailing newline is added (unlike `echo`)

### Error variant

Check if `RuntimeError` already has a suitable variant for shell execution failures. If not, add `ShellError(String)`. The `RuntimeError` enum is in `crates/engine/src/error.rs`.

### Security improvement

Before this change:
- `local.title = "fix: ${var.bug.title}"` where `bug.title` = `foo" && rm -rf /` would produce an unescaped value `fix: foo" && rm -rf /` that gets injected raw into shell commands (because `local.*` is trusted).

After this change:
- `local.title` becomes plain string `fix: foo" && rm -rf /` (no shell syntax)
- When substituted into shell commands via `interpolate_shell`, the `"` is escaped to `\"`, making it safe.

### Workspace/invoke values

`workspace.*` and `invoke.*` values are already plain data (paths, IDs, nonces) — they never contain `$(...)`. Switching them from trusted to escaped is safe because their values don't contain shell-special characters in practice, and if they did, escaping is the correct behavior.

## Verification Plan

1. **Unit tests** (`cargo test -p oj-runbook`): Confirm `interpolate_shell_trusted` is fully removed, `interpolate_shell` works correctly
2. **Integration tests** (`cargo test -p oj-engine`): Confirm locals with `$(...)` are eagerly evaluated to plain values
3. **Full suite** (`make check`): clippy, fmt, all tests, build, audit
4. **Manual smoke test** (optional): Run a pipeline with locals containing `$(git rev-parse --show-toplevel)` and verify the stored value is the resolved path
