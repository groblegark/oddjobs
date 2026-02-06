// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Epic runbook parsing tests: complex multi-step HCL with agents, primes, and lifecycle.

use crate::agent::{AgentAction, Attempts, PrimeDef};
use crate::job::{WorkspaceConfig, WorkspaceType};

const EPIC_HCL: &str = r#"
command "epic" {
  args = "<name> <instructions> [--blocked-by <ids>]"
  run  = { job = "epic" }

  defaults = {
    blocked-by = ""
  }
}

job "epic" {
  name      = "${var.name}"
  vars      = ["name", "instructions", "blocked-by"]
  workspace = "folder"

  locals {
    repo   = "$(git -C ${invoke.dir} rev-parse --show-toplevel)"
    branch = "feature/${var.name}-${workspace.nonce}"
    title  = "feat(${var.name}): ${var.instructions}"
  }

  notify {
    on_start = "Epic started: ${var.name}"
    on_done  = "Epic landed: ${var.name}"
    on_fail  = "Epic failed: ${var.name}"
  }

  step "init" {
    run = <<-SHELL
      git -C "${local.repo}" worktree add -b "${local.branch}" "${workspace.root}" HEAD
    SHELL
    on_done = { step = "decompose" }
  }

  step "decompose" {
    run     = { agent = "decompose" }
    on_done = { step = "build" }
  }

  step "build" {
    run     = { agent = "epic-builder" }
    on_done = { step = "submit" }
  }

  step "submit" {
    run = <<-SHELL
      git add -A
      git diff --cached --quiet || git commit -m "${local.title}"
      test "$(git rev-list --count HEAD ^origin/main)" -gt 0 || { echo "No changes to submit" >&2; exit 1; }
      git -C "${local.repo}" push origin "${local.branch}"
      oj queue push merges --var branch="${local.branch}" --var title="${local.title}"
    SHELL
    on_done = { step = "cleanup" }
  }

  step "cleanup" {
    run = "git -C \"${local.repo}\" worktree remove --force \"${workspace.root}\" 2>/dev/null || true"
  }
}

agent "decompose" {
  run      = "claude --model opus --dangerously-skip-permissions --disallowed-tools ExitPlanMode,EnterPlanMode"
  on_idle  = { action = "gate", run = "test -s .epic-root-id" }
  on_dead  = "fail"

  prime = [
    "wok prime",
    "echo '## Ready Issues'",
    "wok ready",
    "echo '## Project Instructions'",
    "cat CLAUDE.md 2>/dev/null || true",
  ]

  prompt = "Decompose the epic into tasks."
}

agent "epic-builder" {
  run      = "claude --model opus --dangerously-skip-permissions --disallowed-tools ExitPlanMode,EnterPlanMode"
  on_idle  = { action = "gate", run = "root_id=$(cat .epic-root-id) && ! wok tree \"$root_id\" | grep -qE '(todo|doing)'", attempts = "forever" }
  on_dead  = { action = "resume", append = true, message = "Continue working on the epic." }

  prime = [
    "wok prime $(cat .epic-root-id)",
    "echo '## Epic Tree'",
    "wok tree $(cat .epic-root-id)",
    "echo '## Root Issue'",
    "wok show $(cat .epic-root-id)",
  ]

  prompt = "Work through the epic tasks."
}
"#;

fn parse_epic() -> crate::Runbook {
    super::parse_hcl(EPIC_HCL)
}

#[test]
fn command() {
    let runbook = parse_epic();
    let cmd = &runbook.commands["epic"];
    assert!(cmd.run.is_job());
    assert_eq!(cmd.run.job_name(), Some("epic"));
    assert_eq!(cmd.args.positional.len(), 2);
    assert_eq!(cmd.args.positional[0].name, "name");
    assert_eq!(cmd.args.positional[1].name, "instructions");
    assert_eq!(cmd.args.options.len(), 1);
    assert_eq!(cmd.args.options[0].name, "blocked-by");
    assert_eq!(cmd.defaults.get("blocked-by"), Some(&String::new()));
}

#[test]
fn job() {
    let runbook = parse_epic();
    let job = &runbook.jobs["epic"];
    assert_eq!(job.name.as_deref(), Some("${var.name}"));
    assert_eq!(job.vars, vec!["name", "instructions", "blocked-by"]);
    assert_eq!(
        job.workspace,
        Some(WorkspaceConfig::Simple(WorkspaceType::Folder))
    );
    assert_eq!(job.steps.len(), 5);

    // Step names and transitions
    let steps: Vec<_> = job.steps.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(steps, ["init", "decompose", "build", "submit", "cleanup"]);

    assert!(job.steps[0].run.is_shell());
    assert_eq!(
        job.steps[0].on_done.as_ref().map(|t| t.step_name()),
        Some("decompose")
    );

    assert!(job.steps[1].run.is_agent());
    assert_eq!(job.steps[1].agent_name(), Some("decompose"));
    assert_eq!(
        job.steps[1].on_done.as_ref().map(|t| t.step_name()),
        Some("build")
    );

    assert!(job.steps[2].run.is_agent());
    assert_eq!(job.steps[2].agent_name(), Some("epic-builder"));
    assert_eq!(
        job.steps[2].on_done.as_ref().map(|t| t.step_name()),
        Some("submit")
    );

    assert!(job.steps[3].run.is_shell());
    assert_eq!(
        job.steps[3].on_done.as_ref().map(|t| t.step_name()),
        Some("cleanup")
    );

    assert!(job.steps[4].run.is_shell());
    assert!(job.steps[4].on_done.is_none()); // terminal step

    // Locals
    assert!(job.locals.contains_key("repo"));
    assert!(job.locals.contains_key("branch"));
    assert!(job.locals.contains_key("title"));

    // Notify
    assert!(job.notify.on_start.is_some());
    assert!(job.notify.on_done.is_some());
    assert!(job.notify.on_fail.is_some());
}

#[test]
fn decompose_agent() {
    let runbook = parse_epic();
    let agent = runbook.get_agent("decompose").unwrap();
    assert!(agent.run.contains("claude"));
    assert!(agent.run.contains("--disallowed-tools"));
    assert!(agent.prompt.is_some());

    // on_idle = gate with shell check
    assert_eq!(agent.on_idle.action(), &AgentAction::Gate);
    assert_eq!(agent.on_idle.run(), Some("test -s .epic-root-id"));

    // on_dead = fail
    assert_eq!(agent.on_dead.action(), &AgentAction::Fail);

    // prime = array of 5 commands
    match &agent.prime {
        Some(PrimeDef::Commands(cmds)) => assert_eq!(cmds.len(), 5),
        other => panic!("expected PrimeDef::Commands, got {:?}", other),
    }
}

#[test]
fn builder_agent() {
    let runbook = parse_epic();
    let agent = runbook.get_agent("epic-builder").unwrap();
    assert!(agent.run.contains("claude"));
    assert!(agent.run.contains("--disallowed-tools"));
    assert!(agent.prompt.is_some());

    // on_idle = gate with attempts = forever
    assert_eq!(agent.on_idle.action(), &AgentAction::Gate);
    assert!(agent.on_idle.run().is_some());
    assert_eq!(agent.on_idle.attempts(), Attempts::Forever);

    // on_dead = recover with append
    assert_eq!(agent.on_dead.action(), &AgentAction::Resume);
    assert!(agent.on_dead.append());
    assert!(agent.on_dead.message().is_some());

    // prime = array of 5 commands
    match &agent.prime {
        Some(PrimeDef::Commands(cmds)) => assert_eq!(cmds.len(), 5),
        other => panic!("expected PrimeDef::Commands, got {:?}", other),
    }
}
