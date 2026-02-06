// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Multi-format parsing tests: TOML, JSON, and HCL.

use crate::{parse_runbook, parse_runbook_with_format, Format, Runbook};

/// Shared assertions for the "build" sample runbook across all three formats.
fn assert_sample_build_runbook(runbook: &Runbook) {
    // Command
    let cmd = &runbook.commands["build"];
    assert!(cmd.run.is_job());
    assert_eq!(cmd.run.job_name(), Some("build"));
    assert_eq!(cmd.args.positional.len(), 2);
    assert_eq!(cmd.args.positional[0].name, "name");
    assert_eq!(cmd.args.positional[1].name, "prompt");
    assert_eq!(cmd.defaults.get("branch"), Some(&"main".to_string()));

    // Job
    let job = &runbook.jobs["build"];
    assert_eq!(job.vars, vec!["name", "prompt"]);
    assert_eq!(job.steps.len(), 5);
    assert_eq!(job.steps[0].name, "init");
    assert!(job.steps[0].run.is_shell());
    assert_eq!(job.steps[1].name, "plan");
    assert!(job.steps[1].run.is_agent());
    assert_eq!(job.steps[1].agent_name(), Some("planner"));

    // Agent
    let agent = &runbook.agents["planner"];
    assert!(agent.run.contains("claude"));
    assert!(agent.env.contains_key("OJ_STEP"));
}

// ============================================================================
// TOML Format
// ============================================================================

const SAMPLE_TOML: &str = r#"
[command.build]
args = "<name> <prompt>"
run = { job = "build" }
[command.build.defaults]
branch = "main"

[job.build]
vars  = ["name", "prompt"]

[[job.build.step]]
name = "init"
run = "git worktree add worktrees/${name} -b feature/${name}"
on_done = "plan"

[[job.build.step]]
name = "plan"
run = { agent = "planner" }
on_done = "execute"

[[job.build.step]]
name = "execute"
run = { agent = "executor" }
on_done = "done"
on_fail = "failed"

[[job.build.step]]
name = "done"
run = "echo done"

[[job.build.step]]
name = "failed"
run = "echo failed"

[agent.planner]
run = "claude -p"
prompt = "Plan: ${var.prompt}"
[agent.planner.env]
OJ_STEP = "plan"

[agent.executor]
run = "claude \"${prompt}\""
cwd = "worktrees/${name}"
"#;

#[test]
fn toml_sample_runbook() {
    assert_sample_build_runbook(&parse_runbook(SAMPLE_TOML).unwrap());
}

#[test]
fn toml_empty() {
    let runbook = parse_runbook("").unwrap();
    assert!(runbook.commands.is_empty());
    assert!(runbook.jobs.is_empty());
}

#[test]
fn toml_command_with_args_string() {
    let toml = r#"
[command.deploy]
args = "<env> [-t/--tag <version>] [-f/--force] [targets...]"
run = "deploy.sh"
[command.deploy.defaults]
tag = "latest"
"#;
    let runbook = parse_runbook(toml).unwrap();
    let cmd = &runbook.commands["deploy"];

    assert_eq!(cmd.args.positional.len(), 1);
    assert_eq!(cmd.args.positional[0].name, "env");
    assert_eq!(cmd.args.options.len(), 1);
    assert_eq!(cmd.args.options[0].name, "tag");
    assert_eq!(cmd.args.flags.len(), 1);
    assert_eq!(cmd.args.flags[0].name, "force");
    assert!(cmd.args.variadic.is_some());
    assert_eq!(cmd.args.variadic.as_ref().unwrap().name, "targets");

    assert!(cmd.run.is_shell());
    assert_eq!(cmd.run.shell_command(), Some("deploy.sh"));
}

// ============================================================================
// JSON Format
// ============================================================================

const SAMPLE_JSON: &str = r#"
{
  "command": {
    "build": {
      "args": "<name> <prompt>",
      "run": { "job": "build" },
      "defaults": {
        "branch": "main"
      }
    }
  },
  "job": {
    "build": {
      "input": ["name", "prompt"],
      "step": [
        {
          "name": "init",
          "run": "git worktree add worktrees/${name} -b feature/${name}",
          "on_done": "plan"
        },
        {
          "name": "plan",
          "run": { "agent": "planner" },
          "on_done": "execute"
        },
        {
          "name": "execute",
          "run": { "agent": "executor" },
          "on_done": "done",
          "on_fail": "failed"
        },
        {
          "name": "done",
          "run": "echo done"
        },
        {
          "name": "failed",
          "run": "echo failed"
        }
      ]
    }
  },
  "agent": {
    "planner": {
      "run": "claude -p \"Plan: ${prompt}\"",
      "env": {
        "OJ_STEP": "plan"
      }
    },
    "executor": {
      "run": "claude \"${prompt}\"",
      "cwd": "worktrees/${name}"
    }
  }
}
"#;

#[test]
fn json_sample_runbook() {
    let runbook = super::parse_json(SAMPLE_JSON);
    assert_sample_build_runbook(&runbook);

    // JSON-specific: verify step transitions
    let job = &runbook.jobs["build"];
    assert_eq!(
        job.steps[2].on_done.as_ref().map(|t| t.step_name()),
        Some("done")
    );
    assert_eq!(
        job.steps[2].on_fail.as_ref().map(|t| t.step_name()),
        Some("failed")
    );
    assert_eq!(job.steps[3].name, "done");
    assert_eq!(job.steps[4].name, "failed");
}

#[test]
fn json_empty() {
    let runbook = super::parse_json("{}");
    assert!(runbook.commands.is_empty());
    assert!(runbook.jobs.is_empty());
}

// ============================================================================
// HCL Format
// ============================================================================

const SAMPLE_HCL: &str = r#"
command "build" {
  args = "<name> <prompt>"
  run  = { job = "build" }

  defaults = {
    branch = "main"
  }
}

job "build" {
  vars  = ["name", "prompt"]

  step "init" {
    run     = "git worktree add worktrees/${name} -b feature/${name}"
    on_done = "plan"
  }

  step "plan" {
    run     = { agent = "planner" }
    on_done = "execute"
  }

  step "execute" {
    run     = { agent = "executor" }
    on_done = "done"
    on_fail = "failed"
  }

  step "done" {
    run = "echo done"
  }

  step "failed" {
    run = "echo failed"
  }
}

agent "planner" {
  run = "claude -p \"Plan: ${prompt}\""

  env = {
    OJ_STEP = "plan"
  }
}

agent "executor" {
  run = "claude \"${prompt}\""
  cwd = "worktrees/${name}"
}
"#;

#[test]
fn hcl_sample_runbook() {
    let runbook = super::parse_hcl(SAMPLE_HCL);
    assert_sample_build_runbook(&runbook);

    // HCL-specific: verify step transitions
    let job = &runbook.jobs["build"];
    assert_eq!(
        job.steps[2].on_done.as_ref().map(|t| t.step_name()),
        Some("done")
    );
    assert_eq!(
        job.steps[2].on_fail.as_ref().map(|t| t.step_name()),
        Some("failed")
    );
    assert_eq!(job.steps[3].name, "done");
    assert_eq!(job.steps[4].name, "failed");
}

#[test]
fn hcl_empty() {
    let runbook = parse_runbook_with_format("", Format::Hcl).unwrap();
    assert!(runbook.commands.is_empty());
    assert!(runbook.jobs.is_empty());
}

#[test]
fn hcl_step_names_from_block_labels() {
    let hcl = r#"
job "deploy" {
  vars  = ["env"]

  step "build" {
    run     = "make build"
    on_done = "test"
  }

  step "test" {
    run     = "make test"
    on_done = "deploy"
  }

  step "deploy" {
    run = "make deploy"
  }
}
"#;
    let runbook = super::parse_hcl(hcl);
    let job = &runbook.jobs["deploy"];
    assert_eq!(job.steps.len(), 3);
    assert_eq!(job.steps[0].name, "build");
    assert_eq!(job.steps[1].name, "test");
    assert_eq!(
        job.steps[1].on_done.as_ref().map(|t| t.step_name()),
        Some("deploy")
    );
    assert_eq!(job.steps[2].name, "deploy");
}
