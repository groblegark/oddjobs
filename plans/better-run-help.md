# Better Run Help

Improve `oj run` help output: clean up the base listing (remove inline args), make `oj run <command> -h` show per-command help, and extract per-block HCL comments so each command gets its own description.

## 1. Overview

Today `oj run -h` shows every command with its full argument signature inline, making the listing noisy. And `oj run build -h` shows the *same* listing instead of a detailed help page for `build` — clap intercepts `-h` before the run handler can route it.

**Goal:**

```
❯ oj run -h
Usage: oj run <COMMAND>

Commands:
  build          Plan and implement a feature, then submit to the merge queue.
  chore          File a chore and dispatch it to a worker.
  design         Design — standalone crew agent for collaborative feature design.
  draft          Plan and implement exploratory work, pushed to a draft branch.
  draft-rebase   Rebase a draft branch onto its base, with agent conflict resolution.
  draft-refine   Refine an existing draft branch with additional instructions.
  drafts         List open draft branches, or close one.
  epic           Decompose work into issues and build them all.
  fix            File a bug and dispatch it to a fix worker.
  merge          Queue a branch for the local merge queue.

For more information, try 'oj run <COMMAND> -h'.

❯ oj run build -h
Plan and implement a feature, then submit to the merge queue.

Usage: oj run build <name> <instructions> [--base <branch>]

Arguments:
  <name>
  <instructions>

Options:
  --base <base>            [default: main]

Description:
  Prereq: configure sccache in .cargo/config.toml ...
  ...
  Examples:
    oj run build auth "Add user authentication with JWT tokens"
    oj run build dark-mode "Implement dark mode theme" --base develop
```

Three things need to change:
1. **Per-block comment extraction** — so `draft-rebase` gets its own description instead of `draft`'s file-level comment.
2. **Clean listing** — `oj run -h` shows only command names and descriptions.
3. **Per-command help routing** — `oj run build -h` shows the detailed help page.

## 2. Project Structure

```
crates/
├── runbook/src/
│   ├── find.rs          # Add extract_block_comments(), update collect_all_commands
│   ├── find_tests.rs    # Tests for per-block comment extraction
│   ├── help.rs          # No changes needed (format_command_help already works)
│   ├── help_tests.rs    # No changes needed
│   └── lib.rs           # Export extract_block_comments if needed
└── cli/src/
    ├── main.rs           # Route "run <command>" help to per-command help
    └── commands/
        ├── run.rs        # Clean up format_available_commands, remove args from listing
        └── run_tests.rs  # Update test expectations
```

## 3. Dependencies

No new external dependencies. Uses existing `hcl`, `serde`, regex-free text scanning, and standard library.

## 4. Implementation Phases

### Phase 1: Per-block comment extraction

**File:** `crates/runbook/src/find.rs`

Add a function that scans raw HCL content and extracts comment blocks preceding each `command "name"` block. This is the "post-serde" enrichment the issue describes — serde handles structural parsing, then a text-scanning pass adds comment metadata.

```rust
/// Extract comment blocks preceding each `command "name"` block in HCL content.
///
/// Scans the raw text (not the HCL AST) for lines matching `command "name" {`
/// and collects the preceding `#`-comment block for each.
///
/// Returns a map of command_name → FileComment.
pub fn extract_block_comments(content: &str) -> HashMap<String, FileComment> {
    let lines: Vec<&str> = content.lines().collect();
    let mut result = HashMap::new();

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();

        // Match: command "name" { (with optional trailing content)
        if !trimmed.starts_with("command ") {
            continue;
        }
        let rest = trimmed.strip_prefix("command ").unwrap().trim();
        let name = match rest.strip_prefix('"') {
            Some(after_quote) => {
                match after_quote.find('"') {
                    Some(end) => &after_quote[..end],
                    None => continue,
                }
            }
            None => continue,
        };

        // Walk backwards from line i-1 collecting # comment lines.
        // Stop at a non-comment, non-blank line or at the start of file.
        let mut comment_lines = Vec::new();
        let mut j = i;
        while j > 0 {
            j -= 1;
            let prev = lines[j].trim();
            if prev.starts_with('#') {
                let text = prev
                    .strip_prefix("# ")
                    .unwrap_or(prev.strip_prefix('#').unwrap_or(""));
                comment_lines.push(text);
            } else if prev.is_empty() {
                if comment_lines.is_empty() {
                    continue; // skip blanks between block and comment
                } else {
                    break; // blank line above the comment block = stop
                }
            } else {
                break; // hit a non-comment line (e.g., closing brace of previous block)
            }
        }
        comment_lines.reverse();

        if comment_lines.is_empty() {
            continue;
        }

        // Split into short/long on first blank comment line
        let owned: Vec<String> = comment_lines.iter().map(|s| s.to_string()).collect();
        let split_pos = owned.iter().position(|l| l.is_empty());
        let (short_lines, long_lines) = match split_pos {
            Some(pos) => (&owned[..pos], &owned[pos + 1..]),
            None => (owned.as_slice(), &[][..]),
        };

        result.insert(name.to_string(), FileComment {
            short: short_lines.join("\n"),
            long: long_lines.join("\n"),
        });
    }

    result
}
```

**Key design decisions:**
- Regex-free: simple `starts_with` + `strip_prefix` matching avoids adding a regex dependency.
- Walks backwards from each `command` line to collect the preceding comment block.
- Blank lines between the comment and the `command` line are skipped (common formatting).
- Blank lines *above* the comment block stop collection (prevents picking up unrelated comments or section separators).
- Falls through gracefully for commands with no comment.

**Verification:** Unit tests in `find_tests.rs`:

```rust
#[test]
fn extract_block_comments_multi_command_file() {
    let content = r#"# First command description.
#
# Examples:
#   oj run first

command "first" {
  run = "echo first"
}

# Second command description.
command "second" {
  run = "echo second"
}
"#;
    let comments = extract_block_comments(content);
    assert_eq!(comments.len(), 2);
    assert_eq!(comments["first"].short, "First command description.");
    assert!(comments["first"].long.contains("Examples:"));
    assert_eq!(comments["second"].short, "Second command description.");
    assert!(comments["second"].long.is_empty());
}

#[test]
fn extract_block_comments_no_comment() {
    let content = r#"command "bare" { run = "echo" }"#;
    let comments = extract_block_comments(content);
    assert!(comments.is_empty());
}

#[test]
fn extract_block_comments_ignores_section_separators() {
    // The "# ---" separator should not bleed into the second command's comment
    let content = r#"# First description
command "first" {
  run = "echo"
}

# ------------------------------------------------------------------
# Agents
# ------------------------------------------------------------------

# Second description
command "second" {
  run = "echo"
}
"#;
    let comments = extract_block_comments(content);
    assert_eq!(comments["second"].short, "Second description");
}
```

### Phase 2: Update command collection and lookup to use per-block comments

**File:** `crates/runbook/src/find.rs`

Update `collect_all_commands` to use `extract_block_comments` for per-command descriptions instead of applying the file-level comment to all commands:

```rust
pub fn collect_all_commands(runbook_dir: &Path) -> Result<Vec<(String, CommandDef)>, FindError> {
    // ... existing file scanning ...
    for (path, format) in files {
        let content = /* read file */;
        let runbook = /* parse */;

        // NEW: per-block comments instead of file-level comment
        let block_comments = extract_block_comments(&content);
        let file_comment = extract_file_comment(&content);

        for (name, mut cmd) in runbook.commands {
            if cmd.description.is_none() {
                // Prefer per-block comment; fall back to file-level comment
                let comment = block_comments.get(&name).or(file_comment.as_ref());
                if let Some(comment) = comment {
                    let desc_line = comment.short.lines()
                        .nth(1)
                        .or_else(|| comment.short.lines().next())
                        .unwrap_or("");
                    if !desc_line.is_empty() {
                        cmd.description = Some(desc_line.to_string());
                    }
                }
            }
            commands.push((name, cmd));
        }
    }
    // ...
}
```

Update `find_command_with_comment` similarly — use per-block comment for the specific command rather than file-level comment:

```rust
pub fn find_command_with_comment(
    runbook_dir: &Path,
    command_name: &str,
) -> Result<Option<(CommandDef, Option<FileComment>)>, FindError> {
    // ... existing scanning ...
    if let Some(cmd) = runbook.commands.get(command_name) {
        let block_comments = extract_block_comments(&content);
        let comment = block_comments.remove(command_name)
            .or_else(|| extract_file_comment(&content));
        return Ok(Some((cmd.clone(), comment)));
    }
    // ...
}
```

**Verification:**

```rust
#[test]
fn collect_all_commands_per_block_descriptions() {
    let tmp = TempDir::new().unwrap();
    let content = r#"# First command
command "alpha" {
  run = "echo alpha"
}

# Second command
command "beta" {
  run = "echo beta"
}
"#;
    write_hcl(tmp.path(), "multi.hcl", content);
    let commands = collect_all_commands(tmp.path()).unwrap();
    assert_eq!(commands[0].1.description.as_deref(), Some("First command"));
    assert_eq!(commands[1].1.description.as_deref(), Some("Second command"));
}
```

Export `extract_block_comments` from `lib.rs` if other crates need it; otherwise keep it `pub(crate)`.

### Phase 3: Clean up `oj run -h` listing (remove inline args)

**File:** `crates/cli/src/commands/run.rs`

Change `format_available_commands` to show only command names and descriptions — no argument signatures:

```rust
fn format_available_commands(
    help: &mut crate::color::HelpPrinter,
    commands: &[(String, oj_runbook::CommandDef)],
) {
    help.usage("oj run <COMMAND>");       // ← was "oj run <COMMAND> [ARGS]..."
    help.blank();

    if commands.is_empty() {
        help.plain("No commands found.");
        help.plain("Define commands in .oj/runbooks/*.hcl");
    } else {
        help.header("Commands:");
        for (name, cmd) in commands {
            // Just the name, no args — args go in per-command help
            help.entry(name, 20, cmd.description.as_deref());
        }
    }

    help.blank();
    help.hint("For more information, try 'oj run <COMMAND> -h'.");  // ← updated hint
}
```

Changes:
- Usage line: `oj run <COMMAND>` (drop `[ARGS]...`)
- Entry: just `name` (drop `cmd.args.usage_line()`)
- Column width: 20 (down from 40, since names without args are shorter)
- Footer hint: `oj run <COMMAND> -h` (guide user to per-command help)

**File:** `crates/cli/src/commands/run_tests.rs`

Update the `format_available_commands_shows_commands` test:

```rust
#[test]
fn format_available_commands_shows_commands() {
    let commands = vec![
        ("build".to_string(), make_shell_command("build", "make build")),
        ("greet".to_string(), make_shell_command_with_args("greet", "<name>", "echo ${args.name}")),
    ];

    let mut help = HelpPrinter::uncolored();
    format_available_commands(&mut help, &commands);
    let buf = help.finish();

    assert!(buf.contains("Commands:"));
    assert!(buf.contains("build"));
    assert!(buf.contains("greet"));
    assert!(!buf.contains("<name>"));  // ← args no longer shown
    assert!(!buf.contains("No commands found."));
}
```

Also update the `format_available_commands_empty_shows_no_commands` test to check for `Usage: oj run <COMMAND>\n` (without `[ARGS]...`).

**Verification:** Run tests, then manual check: `oj run -h` shows the clean listing.

### Phase 4: Route `oj run <command> -h` to per-command help

**File:** `crates/cli/src/main.rs`

The problem: clap intercepts `-h` via `DisplayHelp` before `run.rs::handle()` ever sees it. The interception at `print_formatted_help` finds the `run` subcommand (via `find_subcommand`) and prints its `override_help` — the full listing.

**Fix:** In `print_formatted_help`, detect when the user asked for help on a specific `run` command and route to per-command help instead.

```rust
fn print_formatted_help(args: &[String]) {
    let cmd = cli_command();

    let non_flags: Vec<&String> = args
        .iter()
        .skip(1)
        .filter(|arg| !arg.starts_with('-'))
        .collect();

    let subcommand_names: Vec<&str> = if non_flags.first().map(|s| s.as_str()) == Some("help") {
        non_flags.iter().skip(1).map(|s| s.as_str()).collect()
    } else {
        non_flags.iter().map(|s| s.as_str()).collect()
    };

    // NEW: Route "run <command>" to per-command help
    if subcommand_names.first() == Some(&"run") && subcommand_names.len() > 1 {
        let command_name = subcommand_names[1];
        let project_root = find_project_root_from_args();
        let runbook_dir = project_root.join(".oj/runbooks");

        // Try to load and display per-command help
        if let Ok(Some(runbook)) =
            oj_runbook::find_runbook_by_command(&runbook_dir, command_name)
        {
            if let Some(cmd_def) = runbook.get_command(command_name) {
                let comment = oj_runbook::find_command_with_comment(&runbook_dir, command_name)
                    .ok()
                    .flatten()
                    .and_then(|(_, comment)| comment);

                eprint!("{}", cmd_def.format_help(command_name, comment.as_ref()));
                return;
            }
        }
        // Fall through to normal help if command not found
    }

    let mut target_cmd = find_subcommand(cmd, &subcommand_names);
    help::print_help(&mut target_cmd);
}
```

This keeps the existing mechanism for all other subcommands and only intercepts the `run <command>` pattern. If the command doesn't exist in any runbook, it falls through to the normal help display (which shows the listing).

**Note:** This means the per-command help for `oj run build -h` goes through two code paths:
1. When clap intercepts `-h` → `print_formatted_help` → new routing code above
2. When trailing_var_arg captures `-h` → `run.rs::handle()` → `print_command_help()`

Both paths produce the same output via `cmd_def.format_help()`. Path (2) is the fallback; the code at `run.rs:149` can remain as-is for robustness.

**Verification:** Manual tests:
- `oj run build -h` → per-command help for build
- `oj run draft -h` → per-command help for draft
- `oj run nonexistent -h` → falls through to listing
- `oj run -h` → clean listing
- `oj run` → clean listing
- `oj run --help` → clean listing

### Phase 5: Tests and polish

1. **Run `make check`** — fmt, clippy, tests, build, deny.
2. **Update any snapshot or assertion tests** that check the old help format.
3. **Verify per-block comments** with the real `.oj/runbooks/draft.hcl` (4 commands, each with its own comment).

Specific tests to add or update:

| File | Test | What it verifies |
|------|------|------------------|
| `find_tests.rs` | `extract_block_comments_multi_command_file` | Per-block extraction with multiple commands |
| `find_tests.rs` | `extract_block_comments_no_comment` | Command with no preceding comment |
| `find_tests.rs` | `extract_block_comments_ignores_section_separators` | Section separator doesn't bleed into next command |
| `find_tests.rs` | `extract_block_comments_blank_lines_between` | Blank lines between comment and command block |
| `find_tests.rs` | `collect_all_commands_per_block_descriptions` | Each command gets its own description |
| `find_tests.rs` | `find_command_with_comment_uses_block_comment` | Per-block comment returned for specific command |
| `run_tests.rs` | `format_available_commands_shows_commands` | Updated: no args in listing |
| `run_tests.rs` | `format_available_commands_empty_shows_no_commands` | Updated: new usage line |

## 5. Key Implementation Details

### Comment extraction is text-based, not AST-based

The `extract_block_comments` function scans raw text, not the HCL AST. This is the "post-serde" approach described in the issue: serde handles all structural parsing (commands, args, pipelines), while a separate text pass enriches with comments. This keeps the serde layer simple and maintainable.

The hcl-rs crate's serde interface does not expose comments, so text scanning is the only option without switching to a lower-level HCL parser.

### Backward walk algorithm

For each `command "name" {` line, the algorithm walks backward to collect `#` comment lines:
- Blank lines immediately above the command line are skipped (common formatting)
- Once comment lines are found, a blank line *above* them stops collection
- Non-comment, non-blank lines (e.g., `}` from a previous block) stop collection

This handles the common patterns in `.oj/runbooks/draft.hcl`:
```hcl
command "draft" { ... }

# Rebase a draft branch onto its base.     ← collected for draft-rebase
command "draft-rebase" { ... }
```

### Description extraction priority

For the short description shown in the listing:
1. Explicit `description` field in HCL (highest priority)
2. Per-block comment (second line of short, or first line if single-line)
3. File-level comment (fallback for files with one command and no per-block match)

### Help routing priority

For `-h`/`--help`:
1. Clap intercepts and calls `print_formatted_help`
2. If args match `run <command>`, route to per-command help
3. If command not found in runbooks, fall through to listing
4. For non-run subcommands, use clap's normal help

### TOML/JSON runbooks

Per-block comment extraction only works for HCL files (TOML and JSON don't have standardized comment preservation). For TOML/JSON runbooks, users should set the `description` field explicitly. HCL is the recommended format per the project docs.

## 6. Verification Plan

1. **Unit tests** (Phase 1-2): per-block comment extraction, command collection with per-block descriptions
2. **Unit tests** (Phase 3): updated help listing format assertions
3. **Integration** (Phase 4): `oj run build -h` produces per-command help (manual or e2e test)
4. **Regression**: existing `help_tests.rs` and `find_tests.rs` tests still pass
5. **`make check`** must pass (fmt, clippy, all tests, build, cargo deny)
6. **Manual smoke test**: `oj run -h`, `oj run build -h`, `oj run draft-rebase -h`, `oj run nonexistent -h`
