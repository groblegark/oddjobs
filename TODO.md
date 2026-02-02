# TODO

In progress:
  - fix(cli): oj pipeline wait Ctrl+C handling (agent working)

Backlog (roughly priority-ordered):

Core pipeline
  1. Worker poll interval: optional poll_interval on worker blocks for periodic checks
  2. Merge pipeline: handle non-fast-forward push (rebase-before-push or retry)

Reliability
  3. Cron: watchdog and janitor runbooks for stuck agents / stale resources
  4. Default error handling for agent errors (rate limit → retry, no internet → retry,
     out of credits → escalate, unauthorized → escalate). See design notes below.

Human-in-the-loop
  5. Status: quick status command showing active/escalated pipelines, queued merges,
     active agents, enabled crons
  6. Human In The Loop: CLI-first commands for handling escalation
  7. Terminal: Dedicated fullscreen TUI for "watching" status and interactive inbox

CLI polish
  8. CLI color output: detect tty, respect COLOR/NO_COLOR env vars
      - Copy color conventions from ../quench and ../wok
      - Colorize show, list, --help views and other human-facing output
      - Ask human for preferences on color scheme before implementing

Multi-project
  9. Shared queues: allow cross-project queue push (e.g. --project flag routing)
  10. Remote daemon: coordinate jobs across multiple machines

----

Key files:
  .oj/runbooks/bug.hcl            — fix worker pipeline
  .oj/runbooks/build.hcl          — feature build pipeline
  .oj/runbooks/chore.hcl          — chore worker pipeline
  .oj/runbooks/merge.hcl          — local merge queue
  crates/runbook/src/find.rs      — runbook discovery (recursive scanner)
  crates/runbook/src/queue.rs     — QueueDef, QueueType, RetryConfig
  crates/engine/src/spawn.rs      — agent spawn, prompt interpolation
  crates/engine/src/workspace.rs  — agent settings injection (Stop, Notification hooks)
  crates/engine/src/monitor.rs    — MonitorState, PromptType, action effects
  crates/engine/src/runtime/handlers/worker.rs — worker lifecycle, dead letter
  crates/daemon/src/listener/     — request handlers (workers, queues, commands)
  crates/daemon/src/lifecycle.rs   — event processing, WAL persistence, state materialization

----

## Design Notes

### Default Error Handling

Add sensible default `on_error` behavior for each `AgentError` variant so
agents recover automatically without requiring explicit runbook configuration.

Currently all four error types are categorized as `Failed` states and trigger
`on_error` (default: `"escalate"`). Most of these should retry automatically:

1. **`RateLimited`** — default should auto-retry with jittered backoff
   (e.g. `gate` with `run = "sleep $((60 + RANDOM % 30))"`,
   `attempts = "forever"`, `cooldown = "60s"`). Rate limits are transient
   and almost always resolve on their own.

2. **`NoInternet`** — default should auto-retry with jittered backoff
   (e.g. `gate` with `run = "sleep $((15 + RANDOM % 30))"`,
   `attempts = "forever"`, `cooldown = "30s"`). Network blips are transient;
   escalating immediately is too aggressive.

3. **`OutOfCredits`** — default should escalate. Requires human intervention.

4. **`Unauthorized`** — default should escalate (or fail). Invalid API key
   won't fix itself.

Open questions:
- Should the defaults be baked into the engine (hardcoded fallback when no
  `on_error` is configured), or expressed as default runbook values?
- What backoff strategy for rate limits? Fixed delay, exponential, or
  adaptive based on the retry-after header (if available in the log)?
- Should `NoInternet` have a max attempt count before escalating, or retry
  indefinitely on the assumption connectivity will return?

----

## Dogfooding Notes

Landed:
  - fix(engine): resume from terminal failure resets to failed step
  - fix(engine): shell-safe interpolation (interpolate_shell) for shell commands
  - fix(engine): agent idle/nudge/gate logging in pipeline logs and daemon traces
  - fix(runbooks): switch build/bugfix agents to nudge on_idle (not gate)
  - docs: agent entity description, notify block syntax in runbook CLAUDE.md
  - fix(engine): agent:signal complete overrides gate escalation (race condition fix)
  - fix(specs): resilient binary resolution for shared target dirs
  - fix(daemon): restore WorkerStop handler lost in merge
  - fix(cli): resolve worktree paths to main project root (merge dispatch fix)
  - fix(runbooks): use .cargo/config.toml instead of target symlink
  - fix(engine): kill tmux sessions on pipeline step transitions and failure
  - fix(engine): add tracing to persisted queue poll and worker wake flow
  - fix(runbooks): worktree init, feature branches with nonce, JSON queue push syntax
  - fix(runbooks): git -C $REPO push for worktree submit, wok done from invoke.dir
  - fix(cli): oj queue push uses positional queue name + JSON
  - fix(runbook): scanner recurses into subdirectories
  - fix(engine): reload runbook from disk on each worker poll (stale runbook fix)
  - fix(engine): enforce worker concurrency after daemon restart
  - fix(daemon): materialize WAL events so queries see runtime state
  - feat(worker-stop): oj worker stop command
  - feat(queue-items): oj queue items <queue> command
  - feat(resume-var): rename --input to --var on pipeline resume
  - feat(notify): pipeline notify blocks with on_start/on_done/on_fail (via notify-rust)
  - feat(runbook): add notify blocks to bugfix, build, and merge runbooks
  - feat(runbook): pipeline locals {} block for DRY variable definitions
  - feat(worker): make worker start idempotent, remove separate wake command
  - feat(queue): add --var key=value syntax to oj queue push
  - fix(runbook): surface skipped runbook errors when command not found
  - fix(engine): drop stale agent watcher events after pipeline advances
  - fix(submit): use --var syntax in runbook submit steps to avoid shell escaping issues
  - feat(daemon): auto-start workers on persisted queue push
  - feat(cli): oj agent send command for reliable agent messaging
  - feat(cli): oj pipeline wait accepts multiple IDs (any/--all modes)
  - feat(core): per-project namespace isolation (OJ_NAMESPACE env propagation)
  - feat(cli): oj daemon restart convenience command
  - feat(cli): oj pipeline wait streams step progress in real time
  - feat(cli): oj queue drop to remove stale queue items
  - feat(queue): dead letter semantics with retry = { attempts, cooldown }
  - feat(cli): oj queue retry to resurrect dead/failed items
  - chore: tech debt cleanup (namespace key consistency, plan artifact removal)
  - chore(runbooks): add chore runbook to oddjobs, wok, quench
  - chore(runbooks): rename bugfix.hcl → bug.hcl across projects
  - feat(cli): oj pipeline show var truncation (--verbose for full values)
  - chore: update oddjobs docs for recent features
  - chore: update wok docs for recent features (CLAUDE.md, daemon crate, oj workspace section)
  - feat(notify): route notifications through NotifyAdapter trait (testable with FakeNotify)
  - feat(notify): agent lifecycle notifications (on_start, on_done, on_fail)
  - feat(cli): oj agent list command with status/pipeline filters
  - feat(cli): oj run with no args lists available commands from runbooks
  - feat(engine): worker concurrency > 1 (parallel dispatch up to configured limit)
  - feat(runbook): pipeline on_cancel step transition for cleanup on cancellation
  - feat(engine): inject Claude Code Notification hooks for instant idle/prompt detection
  - feat(engine): MonitorState::Prompting with on_prompt agent config (default: escalate)
  - feat(runbook): pipeline name templates (slugified + nonced, 24-char max + stop words)
  - fix(engine): shell interpolation escaping for special chars in local.title
  - fix(runbook): var truncation specifiers (${var:0:60} syntax)
  - chore(cli): dynamic column widths in pipeline/workspace/worker list commands
  - chore(cli): show project name on oj run invocation
  - fix(wok): finish sync architecture rewrite (CLI routes through daemon IPC in user-level mode)

Workflow patterns that work:
  - oj run fix → agent fixes → submit pushes branch + queue push → merge worker
    dispatches merge pipeline. Full loop works end-to-end now.
  - oj run build → plan agent → implement agent → submit → merge. Works end-to-end.
  - oj run chore → agent works → submit → merge. Works end-to-end.
  - Submit steps use --var syntax to avoid shell escaping issues with titles
  - Pipeline locals {} reduce boilerplate (repo, branch, title defined once)
  - Cherry-pick from worktree branch when submit step fails (shell escaping, etc.)
  - Always make install + daemon restart after cherry-picking.
  - Clean up worktrees: oj workspace drop <id>
  - Parallel builds: kick off multiple oj run build concurrently, monitor with
    oj pipeline wait <id1> <id2> <id3> for streaming progress.
  - Manual merge fallback: git fetch + merge when merge queue is stuck.
  - Multi-project: run pipelines across oddjobs, wok, quench simultaneously.
    Each project has its own namespace, workers, and queues.
  - Merge queue handles conflicts automatically via resolve agent.
  - Worker concurrency > 1 landed but not yet tested in practice.
