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
