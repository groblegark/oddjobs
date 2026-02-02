# Formula: Boot Triage — Ephemeral Watchdog
#
# Gas Town equivalent: mol-boot-triage.formula.toml
#
# Boot is the bridge between the mechanical daemon and intelligent agents.
# It runs FRESH each tick — no accumulated context debt. Makes a single
# decision: does the deacon (or any agent) need attention?
#
# Gas Town watchdog chain:
#   Daemon (Go process) — dumb transport, 3-min heartbeat
#     └→ Boot (this) — intelligent triage, fresh each tick
#         └→ Deacon — continuous patrol, long-running
#
# Boot decision matrix:
#   Session dead            → START (respawn)
#   Heartbeat > 15 min      → WAKE  (nudge deacon)
#   Heartbeat 5-15min+mail  → NUDGE (gentle prod)
#   Heartbeat fresh          → NOTHING (exit silently)
#
# Usage:
#   oj run gt-triage

command "gt-triage" {
  args = ""
  run  = { pipeline = "boot-triage" }
}

pipeline "boot-triage" {
  workspace = "ephemeral"

  # Observe system state
  step "observe" {
    run = <<-SHELL
      oj status 2>/dev/null || true

      for TARGET in deacon witness refinery mayor; do
        COUNT=$(bd list -t message --label "to:$TARGET" --status open --json 2>/dev/null | \
          jq 'length' 2>/dev/null || echo 0)
        if [ "$COUNT" -gt 0 ]; then
          echo "$TARGET: $COUNT pending"
        fi
      done

      ESC=$(bd list -t task --label escalation --status open --json 2>/dev/null | \
        jq 'length' 2>/dev/null || echo 0)
      test "$ESC" -gt 0 && echo "escalations: $ESC"

      MQ=$(bd list -t merge-request --status open --json 2>/dev/null | \
        jq 'length' 2>/dev/null || echo 0)
      test "$MQ" -gt 0 && echo "merge-queue: $MQ"
    SHELL
    on_done = { step = "decide" }
  }

  # Make a single triage decision
  step "decide" {
    run     = { agent = "boot-agent" }
  }
}

# Boot agent — ephemeral, fresh context, single decision
#
# Gas Town: "Boot exists because the daemon can't reason and Deacon can't
# observe itself. The separation costs complexity but enables intelligent
# triage without constant AI cost."
agent "boot-agent" {
  run      = "claude --dangerously-skip-permissions"
  on_idle  = { action = "done" }
  on_dead  = { action = "done" }

  env = {
    BD_ACTOR = "boot"
    GT_ROLE  = "boot"
    GT_SCOPE = "town"
  }

  prompt = <<-PROMPT
    You are Boot — the ephemeral triage agent. You run fresh each tick with
    zero accumulated context. Make ONE decision and exit immediately.

    ## Triage Decision Matrix

    Based on the system state shown in your prime context:

    | Condition                  | Action  | How                        |
    |----------------------------|---------|----------------------------|
    | Dead workers needed        | START   | Note what needs starting   |
    | Stale agents + pending mail| NUDGE   | Note who needs nudging     |
    | Unacked escalations        | ALERT   | Note the escalation        |
    | Everything healthy         | NOTHING | Say "All clear, I'm done"  |

    ## Rules

    - Be FAST. Read the state, decide, exit.
    - Don't investigate deeply — that's the deacon's job.
    - If something looks wrong, note it. Don't fix it.
    - Healthy system = exit silently (Idle Town Principle)

    What does the system need right now? Decide and say "I'm done".
  PROMPT
}
