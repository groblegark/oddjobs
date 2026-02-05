# Process GitHub PRs labeled for auto-merge.
#
# Rebases PRs onto main, pushes, and enables auto-merge. GitHub CI runs in the
# cloud — when checks pass, GitHub merges automatically. If rebase conflicts,
# an agent resolves them.
#
# Prerequisites:
#   - GitHub CLI (gh) installed and authenticated
#   - PRs must have the "auto-merge" label
#   - Repository must have auto-merge enabled in settings
#
# Examples:
#   oj worker start github-prs

queue "github-prs" {
  type = "external"
  list = "gh pr list --json number,title,headRefName --label auto-merge"
  take = "echo ${item.number}"
}

worker "github-prs" {
  source      = { queue = "github-prs" }
  handler     = { job = "github-merge" }
  concurrency = 1
}

job "github-merge" {
  name      = "PR #${var.pr.number}: ${var.pr.title}"
  vars      = ["pr"]
  workspace = "folder"
  on_cancel = { step = "cleanup" }

  locals {
    repo   = "$(git -C ${invoke.dir} rev-parse --show-toplevel)"
    branch = "pr-${var.pr.number}-${workspace.nonce}"
  }

  notify {
    on_start = "Rebasing PR #${var.pr.number}: ${var.pr.title}"
    on_done  = "Submitted PR #${var.pr.number}: ${var.pr.title}"
    on_fail  = "Failed PR #${var.pr.number}: ${var.pr.title}"
  }

  step "init" {
    run = <<-SHELL
      git -C "${local.repo}" worktree remove --force "${workspace.root}" 2>/dev/null || true
      git -C "${local.repo}" branch -D "${local.branch}" 2>/dev/null || true
      rm -rf "${workspace.root}" 2>/dev/null || true
      git -C "${local.repo}" fetch origin main
      git -C "${local.repo}" fetch origin pull/${var.pr.number}/head:${local.branch}
      git -C "${local.repo}" worktree add "${workspace.root}" ${local.branch}
    SHELL
    on_done = { step = "rebase" }
  }

  step "rebase" {
    run     = "git rebase origin/main"
    on_done = { step = "push" }
    on_fail = { step = "resolve" }
  }

  step "resolve" {
    run     = { agent = "github-resolver" }
    on_done = { step = "push" }
  }

  step "push" {
    run = <<-SHELL
      git push --force-with-lease origin ${local.branch}:${var.pr.headRefName}
      gh pr merge ${var.pr.number} --squash --auto
    SHELL
    on_done = { step = "cleanup" }
  }

  step "cleanup" {
    run = <<-SHELL
      git -C "${local.repo}" worktree remove --force "${workspace.root}" 2>/dev/null || true
      git -C "${local.repo}" branch -D "${local.branch}" 2>/dev/null || true
    SHELL
  }
}

agent "github-resolver" {
  run      = "claude --model opus --dangerously-skip-permissions"
  on_idle  = { action = "gate", command = "test ! -d $(git rev-parse --git-dir)/rebase-merge" }
  on_dead  = { action = "escalate" }

  prime = [
    "echo '## Git Status'",
    "git status",
    "echo '## PR Info'",
    "gh pr view ${var.pr.number}",
    "echo '## Conflicted Files'",
    "git diff --name-only --diff-filter=U",
  ]

  prompt = <<-PROMPT
    You are resolving rebase conflicts for PR #${var.pr.number}: ${var.pr.title}

    ## Steps

    1. Run `git status` to see conflicted files
    2. Resolve each conflict and `git add` the resolved files
    3. Run `git rebase --continue`
    4. Repeat until rebase completes

    When the rebase is complete (no more conflicts), say "I'm done".
  PROMPT
}
