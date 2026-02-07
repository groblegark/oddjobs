# LEGACY: Local merge queue with conflict resolution.
#
# This library is superseded by the GT refinery, which is the canonical merge
# queue owner in Gas Town. The GT refinery handles priority scoring, strategy
# selection, and merge execution.
#
# This library remains available for non-Gas Town deployments where OJ operates
# standalone. To enable it, set OJ_LEGACY_MERGE=1 in your environment. Without
# that variable, the command and jobs will exit with a warning.
#
# See: od-vq6.2 (merge queue ownership), od-ki9.3 (legacy flagging)
#
# Clean merges flow through the fast-path. Conflicts are forwarded to a
# resolve queue where an agent handles resolution without blocking
# subsequent clean merges.
#
# Consts:
#   check - verification command (default: "true")

const "check" { default = "true" }

# Queue a branch for the local merge queue.
command "merge" {
  args = "<branch> <title> [--base <base>]"
  run  = <<-SHELL
    if [ "${OJ_LEGACY_MERGE:-}" != "1" ]; then
      echo "error: OJ merge queue is legacy. The GT refinery is the canonical merge owner." >&2
      echo "hint:  Set OJ_LEGACY_MERGE=1 to use this runbook in standalone (non-Gas Town) mode." >&2
      exit 1
    fi
    oj queue push merges --var branch="${args.branch}" --var title="${args.title}" --var base="${args.base}"
    echo "Queued '${args.branch}' for merge"
  SHELL

  defaults = {
    base = "main"
  }
}

queue "merges" {
  type     = "persisted"
  vars     = ["branch", "title", "base"]
  defaults = { base = "main" }
}

queue "merge-conflicts" {
  type     = "persisted"
  vars     = ["branch", "title", "base"]
  defaults = { base = "main" }
}

worker "merge" {
  source      = { queue = "merges" }
  handler     = { job = "merge" }
  concurrency = 1
}

worker "merge-conflict" {
  source      = { queue = "merge-conflicts" }
  handler     = { job = "merge-conflict" }
  concurrency = 1
}

# Fast-path: clean merges only. Conflicts get forwarded to the resolve queue.
job "merge" {
  name      = "${var.mr.title}"
  vars      = ["mr"]
  workspace = "folder"
  on_cancel = { step = "cleanup" }

  locals {
    repo   = "$(git -C ${invoke.dir} rev-parse --show-toplevel)"
    branch = "merge-${workspace.nonce}"
  }

  notify {
    on_start = "Merging: ${var.mr.title}"
    on_done  = "Merged: ${var.mr.title}"
    on_fail  = "Merge failed: ${var.mr.title}"
  }

  # Legacy guard: reject merge jobs unless OJ_LEGACY_MERGE=1 is set.
  step "guard" {
    run = <<-SHELL
      if [ "${OJ_LEGACY_MERGE:-}" != "1" ]; then
        echo "error: OJ merge queue is legacy. The GT refinery is the canonical merge owner." >&2
        echo "hint:  Set OJ_LEGACY_MERGE=1 to use this runbook in standalone (non-Gas Town) mode." >&2
        exit 1
      fi
    SHELL
    on_done = { step = "init" }
  }

  step "init" {
    run = <<-SHELL
      git -C "${local.repo}" worktree remove --force "${workspace.root}" 2>/dev/null || true
      git -C "${local.repo}" branch -D "${local.branch}" 2>/dev/null || true
      rm -rf "${workspace.root}" 2>/dev/null || true
      git -C "${local.repo}" ls-remote --exit-code origin "refs/heads/${var.mr.branch}" >/dev/null 2>&1 \
        || { echo "error: branch '${var.mr.branch}' not found on remote"; exit 1; }
      git -C "${local.repo}" fetch origin ${var.mr.base} ${var.mr.branch}
      git -C "${local.repo}" worktree add -b ${local.branch} "${workspace.root}" origin/${var.mr.base}
    SHELL
    on_done = { step = "merge" }
  }

  step "merge" {
    run     = "git merge origin/${var.mr.branch} --no-edit"
    on_done = { step = "push" }
    on_fail = { step = "queue-conflicts" }
  }

  step "queue-conflicts" {
    run = <<-SHELL
      git merge --abort 2>/dev/null || true
      oj queue push merge-conflicts --var branch="${var.mr.branch}" --var title="${var.mr.title}" --var base="${var.mr.base}"
    SHELL
    on_done = { step = "cleanup" }
  }

  step "push" {
    run = <<-SHELL
      git add -A
      git diff --cached --quiet || git commit --amend --no-edit || git commit --no-edit

      # Retry loop: if push fails because main moved, re-fetch and re-merge.
      # Only falls through to on_fail if merging new main conflicts.
      pushed=false
      for attempt in 1 2 3 4 5; do
        git -C "${local.repo}" fetch origin ${var.mr.base}
        git merge origin/${var.mr.base} --no-edit || exit 1
        git -C "${local.repo}" push origin ${local.branch}:${var.mr.base} && pushed=true && break
        echo "push race (attempt $attempt), retrying..."
        sleep 1
      done
      test "$pushed" = true || { echo "error: push failed after 5 attempts"; exit 1; }

      git -C "${local.repo}" push origin --delete ${var.mr.branch} || true
    SHELL
    on_done = { step = "cleanup" }
    on_fail = { step = "init", attempts = 3 }
  }

  step "cleanup" {
    run = <<-SHELL
      git -C "${local.repo}" worktree remove --force "${workspace.root}" 2>/dev/null || true
      git -C "${local.repo}" branch -D "${local.branch}" 2>/dev/null || true
      git -C "${local.repo}" branch -D "${var.mr.branch}" 2>/dev/null || true
    SHELL
  }
}

# Slow-path: agent-assisted conflict resolution.
job "merge-conflict" {
  name      = "Conflicts: ${var.mr.title}"
  vars      = ["mr"]
  workspace = "folder"
  on_cancel = { step = "cleanup" }

  locals {
    repo   = "$(git -C ${invoke.dir} rev-parse --show-toplevel)"
    branch = "merge-${workspace.nonce}"
  }

  notify {
    on_start = "Resolving conflicts: ${var.mr.title}"
    on_done  = "Resolved conflicts: ${var.mr.title}"
    on_fail  = "Conflict resolution failed: ${var.mr.title}"
  }

  # Legacy guard: reject merge-conflict jobs unless OJ_LEGACY_MERGE=1 is set.
  step "guard" {
    run = <<-SHELL
      if [ "${OJ_LEGACY_MERGE:-}" != "1" ]; then
        echo "error: OJ merge queue is legacy. The GT refinery is the canonical merge owner." >&2
        echo "hint:  Set OJ_LEGACY_MERGE=1 to use this runbook in standalone (non-Gas Town) mode." >&2
        exit 1
      fi
    SHELL
    on_done = { step = "init" }
  }

  step "init" {
    run = <<-SHELL
      git -C "${local.repo}" worktree remove --force "${workspace.root}" 2>/dev/null || true
      git -C "${local.repo}" branch -D "${local.branch}" 2>/dev/null || true
      rm -rf "${workspace.root}" 2>/dev/null || true
      git -C "${local.repo}" fetch origin ${var.mr.base} ${var.mr.branch}
      git -C "${local.repo}" worktree add -b ${local.branch} "${workspace.root}" origin/${var.mr.base}
    SHELL
    on_done = { step = "merge" }
  }

  step "merge" {
    run     = "git merge origin/${var.mr.branch} --no-edit"
    on_done = { step = "push" }
    on_fail = { step = "resolve" }
  }

  step "resolve" {
    run     = { agent = "conflicts" }
    on_done = { step = "push" }
  }

  step "push" {
    run = <<-SHELL
      git add -A
      git diff --cached --quiet || git commit --amend --no-edit || git commit --no-edit

      # Retry loop: if push fails because main moved, re-fetch and re-merge.
      # Only falls through to on_fail if merging new main conflicts.
      pushed=false
      for attempt in 1 2 3 4 5; do
        git -C "${local.repo}" fetch origin ${var.mr.base}
        git merge origin/${var.mr.base} --no-edit || exit 1
        git -C "${local.repo}" push origin ${local.branch}:${var.mr.base} && pushed=true && break
        echo "push race (attempt $attempt), retrying..."
        sleep 1
      done
      test "$pushed" = true || { echo "error: push failed after 5 attempts"; exit 1; }

      git -C "${local.repo}" push origin --delete ${var.mr.branch} || true
    SHELL
    on_done = { step = "cleanup" }
    on_fail = { step = "init", attempts = 3 }
  }

  step "cleanup" {
    run = <<-SHELL
      git -C "${local.repo}" worktree remove --force "${workspace.root}" 2>/dev/null || true
      git -C "${local.repo}" branch -D "${local.branch}" 2>/dev/null || true
      git -C "${local.repo}" branch -D "${var.mr.branch}" 2>/dev/null || true
    SHELL
  }
}

agent "conflicts" {
  run      = "claude --model opus --dangerously-skip-permissions"
  on_idle  = { action = "gate", command = "test ! -f $(git rev-parse --git-dir)/MERGE_HEAD" }
  on_dead  = { action = "escalate" }

  session "tmux" {
    color = "blue"
    title = "Merge: ${var.mr.branch}"
    status {
      left  = "${var.mr.title}"
      right = "${var.mr.branch} -> ${var.mr.base}"
    }
  }

  prime = [
    "echo '## Git Status'",
    "git status",
    "echo '## Incoming Commits'",
    "git log origin/${var.mr.base}..origin/${var.mr.branch}",
    "echo '## Changed Files'",
    "git diff --stat origin/${var.mr.base}..origin/${var.mr.branch}",
  ]

  prompt = <<-PROMPT
    You are merging branch ${var.mr.branch} into ${var.mr.base}.

    Title: ${var.mr.title}

    The merge has conflicts that need manual resolution.

    1. Run `git status` to see conflicted files
    2. Resolve the conflicts and `git add` the resolved files
    3. Run `git commit --no-edit` to complete the merge
    4. Verify:
       ```
       ${raw(const.check)}
       ```
    5. Fix any issues
  PROMPT
}
