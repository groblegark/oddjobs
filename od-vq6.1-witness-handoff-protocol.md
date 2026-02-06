# od-vq6.1: Witness Handoff Protocol for OJ-Dispatched Polecats

## Research Spike Report

**Bead:** od-vq6.1
**Date:** 2026-02-06
**Researcher:** Research Agent (od-vq6 subtask)

---

## 1. Executive Summary

When `GT_SLING_OJ=1`, Gas Town's `gt sling` command dispatches polecat lifecycle
to the OJ daemon instead of managing it via tmux directly. However, the per-rig
Witness agent still expects to monitor polecats via tmux sessions. There is no
documented handoff protocol, creating a gap where:

- Witness cannot detect OJ-managed polecat health (no tmux session to query)
- Witness may attempt to nuke an OJ-managed polecat (session not found = "stale")
- POLECAT_DONE messages use a different delivery path (beads mail vs GT mail router)
- Crash recovery is split between two systems with no coordination

This report maps both state machines, identifies all edge cases, and proposes a
concrete implementation plan.

---

## 2. Current State Machines

### 2.1 GT Path: Witness Discovers and Monitors Polecats via tmux

```
  gt sling <bead> <rig>
        |
        v
  [AllocateName] ---- namepool lock, get polecat name
        |
        v
  [polecat.Manager.AddWithOptions] ---- create worktree, agent bead
        |
        v
  [SessionManager.Start] ---- create tmux session "gt-<rig>-<name>"
        |                       set env vars, send startup nudge
        v
  [wakeRigAgents] ---- nudge witness + refinery tmux sessions
        |
        v
  POLECAT RUNNING (tmux session is source of truth)
        |
        |--- Polecat calls "gt done" --->
        |       |
        |       v
        |   [POLECAT_DONE mail] ---- sent via GT mail router
        |       |                     to: <rig>/witness
        |       v
        |   Witness.HandlePolecatDone()
        |       |
        |       +--- has MR? --> create cleanup wisp, wait for MERGED
        |       |
        |       +--- no MR? --> AutoNukeIfClean()
        |               |
        |               +--- cleanup_status=clean --> NukePolecat()
        |               |       |
        |               |       +--- tmux.KillSessionWithProcesses()
        |               |       +--- gt polecat nuke <address>
        |               |
        |               +--- dirty --> create cleanup wisp
        |
        |--- Polecat crashes / daemon shutdown --->
        |       |
        |       v
        |   [LIFECYCLE:Shutdown mail] ---- to: <rig>/witness
        |       |
        |       v
        |   Witness.HandleLifecycleShutdown()
        |       |
        |       +--- stale message? --> ignore
        |       +--- AutoNukeIfClean() (same flow as above)
        |
        |--- Witness patrol detects stale polecat --->
                |
                v
            DetectStalePolecats()
                |
                +--- no tmux session + no agent_state --> stale, nuke
                +--- tmux session active --> not stale
                +--- agent_state=stuck/awaiting-gate --> intentional pause
```

**Key observation:** The GT Witness relies on tmux session existence as the
**source of truth** for polecat liveness (ZFC principle). Every liveness check
goes through `tmux.HasSession()`.

### 2.2 OJ Path: Daemon Monitors Job Lifecycle

```
  gt sling <bead> <rig> (with GT_SLING_OJ=1)
        |
        v
  [dispatchToOj] ---- GT allocates name, calls `oj run gt-sling`
        |               with --var issue, instructions, base, rig,
        |               polecat_name, town_root
        v
  OJ DAEMON receives job
        |
        v
  [sling.provision step] ---- verify bead, create molecule steps,
        |                      hook bead, create worktree
        v
  [sling.execute step] ---- spawn "polecat-worker" agent (Claude)
        |                    OJ manages agent lifecycle:
        |                    - on_idle: nudge
        |                    - on_dead: gate (make check)
        v
  [sling.submit step] ---- commit, push, create MR bead,
        |                   send POLECAT_DONE via `bd create -t message`
        v
  [sling.cleanup step] ---- git worktree remove
        |
        v
  OJ JOB COMPLETE (or FAILED/CANCELLED)
        |
        +--- on_fail: [reopen step] ---- bd reopen <bead>
        +--- on_cancel: [cleanup step] ---- worktree removal
```

**Key observation:** The OJ path sends POLECAT_DONE via `bd create -t message`
with labels `from:<actor>,to:<rig>/witness,msg-type:polecat-done`. This creates
a beads-based message, NOT a GT mail router message. The Witness's inbox check
would need to poll beads for these messages OR they need to be bridged to the
GT mail system.

### 2.3 Gap Analysis: What Happens When OJ Dispatches but Witness Doesn't Know

```
  Current Flow with GT_SLING_OJ=1:

  gt sling
    |
    +--- dispatchToOj() --> oj run gt-sling
    |
    +--- wakeRigAgents() --> nudges witness tmux session
    |
    +--- stores OJ job ID in bead description (storeOjJobIDInBead)
    |
    v
  Witness receives nudge "Polecat dispatched - check for work"
    |
    v
  PROBLEM: Witness looks for tmux session "gt-<rig>-<name>"
           --> NOT FOUND (OJ manages the session, not tmux)
    |
    v
  Witness may classify polecat as:
    - "stale" (no tmux session + no agent_state) --> tries to nuke
    - "done" (no active session) --> tries to clean up
    - Ignores it (if agent bead state is "spawning"/"working")
```

---

## 3. Detailed Edge Case Analysis

### 3.1 OJ Daemon Restarts Mid-Polecat

**Current behavior:**
- OJ daemon persists job state to disk (SQLite). On restart, it resumes jobs.
- The OJ job has `on_cancel = { step = "cleanup" }` and `on_fail = { step = "reopen" }`.
- If daemon restarts during the `execute` step, the Claude agent process dies.
- OJ resumes the job but the agent must be re-spawned.

**Gap:**
- Witness has no visibility into OJ daemon restart events.
- The polecat's agent bead may show `agent_state=working` but the Claude process
  is dead. Witness cannot detect this because there is no tmux session to check.
- OJ's own `on_dead` handler runs `make check` as a gate, but this is local to OJ.

**Risk:** Polecat appears "working" in beads but is actually dead during daemon
restart window. No one detects this until OJ recovers.

### 3.2 Witness Tries to Nuke an OJ-Managed Polecat

**Current behavior in `AutoNukeIfClean` (handlers.go, line 761):**
```go
func AutoNukeIfClean(workDir, rigName, polecatName string) *NukePolecatResult {
    cleanupStatus := getCleanupStatus(workDir, rigName, polecatName)
    switch cleanupStatus {
    case "clean":
        NukePolecat(workDir, rigName, polecatName)  // DANGER
    ...
```

**`NukePolecat` (handlers.go, line 716):**
```go
func NukePolecat(workDir, rigName, polecatName string) error {
    sessionName := fmt.Sprintf("gt-%s-%s", rigName, polecatName)
    t := tmux.NewTmux()
    if running, _ := t.HasSession(sessionName); running {
        t.SendKeysRaw(sessionName, "C-c")
        t.KillSessionWithProcesses(sessionName)
    }
    // Then runs: gt polecat nuke <address>
    util.ExecRun(workDir, "gt", "polecat", "nuke", address)
}
```

**Gap:**
- `NukePolecat` checks for tmux session `gt-<rig>-<name>` -- won't find one for OJ polecats.
- It then runs `gt polecat nuke` which calls `Manager.Remove()`.
- `Remove()` deletes the worktree and closes the agent bead.
- But OJ is STILL RUNNING the job! The worktree gets deleted out from under OJ.
- OJ's execute step (Claude agent) loses its working directory.
- OJ's cleanup step tries to `git worktree remove` on an already-removed path.

**Risk:** CRITICAL. Witness nuking an OJ-managed polecat corrupts the OJ job
and may cause data loss (unpushed commits).

### 3.3 Polecat Crashes -- Who Detects It First?

**GT path:** Witness patrol runs `DetectStalePolecats()` which checks:
1. tmux session existence (source of truth)
2. Agent bead state
3. Commits behind main
4. Uncommitted work

**OJ path:** OJ daemon detects agent death via `on_dead` handler:
1. Agent process exit triggers `AgentFailed` event
2. `on_dead = { action = "gate", run = "make check" }` -- runs gate check
3. If gate passes, step transitions to `on_done` (submits work)
4. If gate fails, step transitions to `on_fail` (reopens bead)

**Gap:**
- OJ detects crash first (it owns the process).
- Witness has no signal that a crash occurred.
- The POLECAT_DONE message is sent by the `submit` step, which only runs if
  the gate passes after agent death. If the agent died before meaningful work,
  no POLECAT_DONE is ever sent.
- Witness would need to discover the polecat's state via beads (the bead may
  be reopened by OJ's `reopen` step, but there's no mail notification).

**Risk:** Polecats that crash and get reopened by OJ are invisible to Witness.
The bead is reopened but Witness never gets a lifecycle message about it.

### 3.4 Mail Delivery: Does POLECAT_DONE Go to Witness on OJ Path?

**GT path mail delivery:**
```go
// In handlers.go, messages arrive via mail.Router
router := mail.NewRouter(townRoot)
// Messages are files in: <townRoot>/<rig>/witness/mail/inbox/
```

**OJ path mail delivery (sling.hcl submit step):**
```bash
bd create -t message \
  --title "POLECAT_DONE ${local.polecat}" \
  --description "Exit: MERGED\nIssue: $BEAD_ID\nBranch: ${local.branch}" \
  --labels "from:${local.actor},to:${var.rig}/witness,msg-type:polecat-done"
```

**Gap:** These are TWO DIFFERENT mail systems:
1. **GT mail router** (`mail.NewRouter`): File-based mailboxes in `<rig>/witness/mail/`
2. **Beads messages** (`bd create -t message`): Beads database entries with label routing

The GT Witness processes mail from `mail.Router`. The OJ sling runbook creates
beads messages. **These do not converge.** The GT Witness will never see the
POLECAT_DONE from an OJ-managed polecat unless:
- The OJ witness-patrol.hcl processes them (it does poll beads messages)
- A bridge is created to forward beads messages to GT mail
- The GT Witness is taught to also poll beads messages

**The OJ witness-patrol.hcl does poll beads messages** (infra.hcl line 62-66):
```hcl
queue "witness-inbox" {
  type = "external"
  list = "bd list -t message --label to:witness --status open --json"
  take = "bd update ${item.id} --status in_progress"
}
```
But this is the OJ witness, not the GT witness. They are separate systems.

### 3.5 Stale Message Detection

**GT path (handlers.go, line 144):**
```go
func isStalePolecatDone(rigName, polecatName string, msg *mail.Message) (bool, string) {
    sessionName := fmt.Sprintf("gt-%s-%s", rigName, polecatName)
    createdAt, err := session.SessionCreatedAt(sessionName)
    // Compares message timestamp to session creation time
}
```

**Gap:** For OJ-managed polecats, `session.SessionCreatedAt()` will fail because
there is no tmux session. This means stale detection is broken -- the Witness
will either accept all messages (if it falls through) or reject all messages
(if the error causes rejection).

Looking at the code, when `session.SessionCreatedAt()` returns an error, the
function returns `false, ""` -- meaning "not stale, allow message." This is
actually the safer default, but it means stale detection is disabled for OJ
polecats.

---

## 4. Implementation Plan

### 4.1 Core Design: OJ-Managed Polecat Registry

The fundamental issue is that Witness uses tmux sessions as the source of truth
for polecat liveness. For OJ-managed polecats, we need an alternative liveness
signal. The design principle should be:

**Beads is truth for OJ polecats; tmux is truth for GT polecats.**

The agent bead already stores `OjJobID` (from `storeOjJobIDInBead` in
sling_oj.go). We can use this as the discriminator.

### 4.2 State Machine Diagram: Handoff Protocol

```
  gt sling (GT_SLING_OJ=1)
        |
        v
  [GT: Validate + Formula + Name Allocation]
        |
        v
  [GT: dispatchToOj()] ----- oj run gt-sling --var ...
        |                           |
        |                           v
        |                     [OJ: Job Created]
        |                           |
        v                           v
  [GT: storeOjJobIDInBead]   [OJ: provision step]
        |                           |
        v                           v
  [GT: wakeRigAgents]        [OJ: execute step (agent)]
        |                           |
        v                           |
  [Witness: Check Inbox]            |
        |                           |
        v                           |
  [Is this an OJ polecat?]         |
        |                           |
   +----+----+                      |
   |         |                      |
  YES       NO                     |
   |         |                      |
   v         v                      |
  [Query    [Legacy              |
   OJ job   tmux                 |
   status]  path]                  |
   |                                |
   v                                v
  [OJ job     [OJ: submit step]
   running?]        |
   |                v
   +--YES-->  [OJ: POLECAT_DONE via bd create]
   |                |
   +--NO--->  [OJ: cleanup/reopen step]
   |                |
   v                v
  [Check      [Witness: Process beads messages]
   beads            |
   for              v
   messages]  [Handle POLECAT_DONE / job failure]
```

### 4.3 Specific Changes

#### Change 1: Add OJ-awareness to Witness liveness checks (MEDIUM)

**Files:**
- `/home/ubuntu/gt11/gastown/internal/witness/handlers.go`
- `/home/ubuntu/gt11/gastown/internal/witness/manager.go` (optional)

**What:**
Before attempting to nuke a polecat, check if it has an `oj_job_id` in its
bead. If it does, query OJ job status via `oj job show <id> --json` instead
of checking tmux.

**Specific changes in `handlers.go`:**

1. **`AutoNukeIfClean` (line 761):** Add early return if polecat is OJ-managed.
   ```go
   func AutoNukeIfClean(workDir, rigName, polecatName string) *NukePolecatResult {
       // NEW: Check if this is an OJ-managed polecat
       if ojJobID := getOjJobID(workDir, polecatName); ojJobID != "" {
           ojStatus := queryOjJobStatus(ojJobID)
           if ojStatus == "running" || ojStatus == "pending" {
               return &NukePolecatResult{
                   Skipped: true,
                   Reason: "OJ-managed polecat (job " + ojJobID + " is " + ojStatus + ")",
               }
           }
           // OJ job completed/failed/cancelled -- safe to check cleanup status
       }
       // ... existing logic
   }
   ```

2. **`NukePolecat` (line 716):** Add OJ job cancellation before nuke.
   ```go
   func NukePolecat(workDir, rigName, polecatName string) error {
       // NEW: Cancel OJ job if this is an OJ-managed polecat
       if ojJobID := getOjJobID(workDir, polecatName); ojJobID != "" {
           exec.Command("oj", "job", "cancel", ojJobID).Run()
       }
       // ... existing tmux kill + gt polecat nuke
   }
   ```

3. **New helper `getOjJobID`:**
   Reads the bead description's `oj_job_id` field. This data is already stored
   by `storeOjJobIDInBead` in `sling_oj.go`.

4. **New helper `queryOjJobStatus`:**
   Runs `oj job show <id> -o json` and parses the status field.

**Complexity:** MEDIUM (4 functions, ~80 lines)
**Dependencies:** Requires `oj` binary accessible to Witness process

#### Change 2: Bridge OJ mail to GT mail system (MEDIUM)

**Files:**
- `/home/ubuntu/gt11/gastown/internal/witness/handlers.go`
- New: `/home/ubuntu/gt11/gastown/internal/witness/oj_bridge.go`

**What:**
Add a periodic check in the Witness patrol that polls beads for
`msg-type:polecat-done` messages addressed to this rig's witness, converts
them to GT mail.Message structs, and processes them through the existing handlers.

**Alternative approach (simpler):** Modify the OJ sling runbook's submit step
to ALSO create a GT mail file in addition to the beads message. This is simpler
but couples the OJ runbook to GT internals.

**Recommended approach:** Teach the GT Witness to ALSO poll beads messages.
Add `PollBeadsInbox()` function that:
1. Runs `bd list -t message --label "to:<rig>/witness" --status open --json`
2. Converts results to `mail.Message` structs
3. Feeds them into the existing `ClassifyMessage` / handler pipeline
4. Closes processed beads messages with `bd close <id>`

**Complexity:** MEDIUM (~60 lines for bridge, reuses existing handlers)
**Dependencies:** `bd` binary accessible to Witness. Beads messages must use
consistent label format.

#### Change 3: Fix stale message detection for OJ polecats (LOW)

**Files:**
- `/home/ubuntu/gt11/gastown/internal/witness/handlers.go`

**What:**
The `isStalePolecatDone` function (line 144) uses `session.SessionCreatedAt()`.
For OJ polecats, enhance to check OJ job creation time instead.

```go
func isStalePolecatDone(rigName, polecatName string, msg *mail.Message) (bool, string) {
    sessionName := fmt.Sprintf("gt-%s-%s", rigName, polecatName)
    createdAt, err := session.SessionCreatedAt(sessionName)
    if err != nil {
        // No tmux session -- check if this is an OJ polecat
        if ojJobID := getOjJobID(...); ojJobID != "" {
            ojCreatedAt := getOjJobCreatedAt(ojJobID)
            if ojCreatedAt.IsZero() {
                return false, "" // Can't determine, allow message
            }
            return session.StaleReasonForTimes(msg.Timestamp, ojCreatedAt)
        }
        return false, "" // existing behavior: allow message
    }
    return session.StaleReasonForTimes(msg.Timestamp, createdAt)
}
```

**Complexity:** LOW (~20 lines)
**Dependencies:** Change 1 (needs `getOjJobID` helper)

#### Change 4: OJ job failure notification to Witness (LOW)

**Files:**
- `/home/ubuntu/gt11/oddjobs/refinery/rig/library/gastown/sling.hcl`
- `/home/ubuntu/gt11/oddjobs/crew/research/.oj/runbooks/gt-sling.hcl`

**What:**
The `reopen` step currently only reopens the bead. It should also send a
lifecycle message to the witness so the witness knows the polecat failed.

```bash
step "reopen" {
    run = <<-SHELL
      BEAD_ID="$(cat ${workspace.root}/.bead-id 2>/dev/null || echo '')"
      test -n "$BEAD_ID" && bd reopen "$BEAD_ID" --reason "Sling job failed" 2>/dev/null || true

      # NEW: Notify witness of polecat failure
      bd create -t message \
        --title "LIFECYCLE:Shutdown ${local.polecat}" \
        --description "Reason: job_failed\nBead: $BEAD_ID" \
        --labels "from:${local.actor},to:${var.rig}/witness,msg-type:lifecycle" \
        2>/dev/null || true
    SHELL
    on_done = { step = "cleanup" }
}
```

**Complexity:** LOW (~5 lines in HCL)
**Dependencies:** Change 2 (Witness must be able to read beads messages)

#### Change 5: Guard `DetectStalePolecats` against OJ polecats (MEDIUM)

**Files:**
- `/home/ubuntu/gt11/gastown/internal/polecat/manager.go`

**What:**
The `DetectStalePolecats` function (line 1495) checks tmux sessions. For OJ
polecats, it should check OJ job status instead.

In `assessStaleness` (line 1569):
```go
func assessStaleness(info *StalenessInfo, threshold int) (bool, string) {
    // NEW: Check if this is an OJ-managed polecat
    if info.OjJobID != "" {
        if info.OjJobStatus == "running" || info.OjJobStatus == "pending" {
            return false, fmt.Sprintf("OJ-managed (job=%s, status=%s)", info.OjJobID, info.OjJobStatus)
        }
        // OJ job completed/failed -- treat as stale candidate
    }
    // ... existing logic
}
```

Also update `StalenessInfo` struct to include `OjJobID` and `OjJobStatus`.

**Complexity:** MEDIUM (~40 lines)
**Dependencies:** Change 1 (needs OJ query helpers)

#### Change 6: Document the protocol (LOW)

**Files:**
- New: protocol documentation section in the library README or a separate doc

**What:**
Document the handoff protocol so both GT and OJ developers understand the
contract. Key sections:
1. Polecat ownership model: GT owns names/beads, OJ owns process/workspace
2. Mail convergence: how messages flow between beads mail and GT mail
3. Liveness checks: tmux for GT polecats, `oj job show` for OJ polecats
4. Nuke safety: always check OJ job status before nuking
5. Crash recovery responsibilities

**Complexity:** LOW (documentation only)

### 4.4 Implementation Priority and Ordering

```
Phase 1 (CRITICAL -- prevents data loss):
  Change 1: OJ-awareness in liveness checks     [MEDIUM, ~80 lines Go]
  Change 5: Guard DetectStalePolecats            [MEDIUM, ~40 lines Go]

Phase 2 (IMPORTANT -- enables Witness monitoring):
  Change 2: Bridge OJ mail to GT mail           [MEDIUM, ~60 lines Go]
  Change 4: OJ job failure notification          [LOW, ~5 lines HCL]

Phase 3 (NICE TO HAVE -- correctness):
  Change 3: Fix stale message detection          [LOW, ~20 lines Go]
  Change 6: Document the protocol                [LOW, documentation]
```

### 4.5 Dependencies on Other od-vq6 Subtasks

| This Change | Depends On | Reason |
|-------------|-----------|--------|
| Change 1 | `oj` binary in PATH | Witness process must be able to call `oj job show` |
| Change 2 | `bd` binary in PATH | Witness must be able to poll beads messages |
| Change 4 | Change 2 | Witness must read beads messages for lifecycle notifications to work |
| Change 5 | Change 1 | Reuses OJ query helpers |
| All | od-vq6 (parent) | Part of OJ-GT convergence epic |

---

## 5. Key Files Reference

### GT (Gastown) Side

| File | Purpose | Key Lines |
|------|---------|-----------|
| `/home/ubuntu/gt11/gastown/internal/witness/manager.go` | Witness lifecycle (start/stop via tmux) | L42-45: `IsRunning` checks tmux session |
| `/home/ubuntu/gt11/gastown/internal/witness/handlers.go` | Protocol message handlers (POLECAT_DONE, MERGED, etc.) | L45: `HandlePolecatDone`, L716: `NukePolecat`, L761: `AutoNukeIfClean` |
| `/home/ubuntu/gt11/gastown/internal/witness/protocol.go` | Message pattern matching and parsing | L14: `PatternPolecatDone`, L17: `PatternLifecycleShutdown` |
| `/home/ubuntu/gt11/gastown/internal/witness/types.go` | Witness config types | L8-23: `WitnessConfig` |
| `/home/ubuntu/gt11/gastown/internal/cmd/sling_oj.go` | OJ dispatch bridge | L37: `dispatchToOj`, L137: `storeOjJobIDInBead` |
| `/home/ubuntu/gt11/gastown/internal/cmd/sling.go` | Unified sling command (both GT and OJ paths) | L604: OJ dispatch branch, L647: legacy tmux branch |
| `/home/ubuntu/gt11/gastown/internal/cmd/sling_helpers.go` | Sling utilities | L680: `wakeRigAgents` |
| `/home/ubuntu/gt11/gastown/internal/polecat/manager.go` | Polecat lifecycle (create/remove/detect stale) | L536: `RemoveWithOptions`, L1495: `DetectStalePolecats` |
| `/home/ubuntu/gt11/gastown/internal/polecat/session_manager.go` | Polecat tmux session management | L152: `Start`, L358: `Stop` |
| `/home/ubuntu/gt11/gastown/internal/polecat/types.go` | Polecat state types and CleanupStatus | L24: State enum, L103: CleanupStatus |
| `/home/ubuntu/gt11/gastown/internal/polecat/pending.go` | Pending spawn discovery from mail | L42: `CheckInboxForSpawns` |

### OJ (Oddjobs) Side

| File | Purpose | Key Lines |
|------|---------|-----------|
| `/home/ubuntu/gt11/oddjobs/crew/research/.oj/runbooks/gt-sling.hcl` | Active OJ sling runbook (dispatched by GT) | L50: provision, L84: execute, L90: submit, L117: reopen |
| `/home/ubuntu/gt11/oddjobs/refinery/rig/library/gastown/sling.hcl` | Library copy of sling runbook | Same structure as above |
| `/home/ubuntu/gt11/oddjobs/refinery/rig/library/gastown/formulas/witness-patrol.hcl` | OJ witness patrol formula | L42: inbox processing, L68: health-scan agent |
| `/home/ubuntu/gt11/oddjobs/refinery/rig/library/gastown/formulas/polecat-work.hcl` | OJ polecat work formula | L57: submit step with POLECAT_DONE |
| `/home/ubuntu/gt11/oddjobs/refinery/rig/library/gastown/infra.hcl` | Shared queues and workers | L62: `witness-inbox` queue (beads-based) |
| `/home/ubuntu/gt11/oddjobs/refinery/rig/library/gastown/start.hcl` | Town startup/shutdown | L50: `gt-stop` sends LIFECYCLE:Shutdown |
| `/home/ubuntu/gt11/oddjobs/mayor/rig/crates/core/src/job.rs` | OJ job state machine (Rust) | L28: StepStatus enum (Pending/Running/Waiting/Completed/Failed) |

### Shared State

| File | Purpose |
|------|---------|
| `/home/ubuntu/gt11/gastown/witness/state.json` | Witness patrol state (patrol_count, active_polecats, notes) |
| Beads database (`.beads/`) | Agent beads with `oj_job_id`, `cleanup_status`, `hook_bead` fields |

---

## 6. Risk Assessment

| Risk | Severity | Likelihood | Mitigation |
|------|----------|-----------|------------|
| Witness nukes OJ-managed polecat mid-work | CRITICAL | HIGH (will happen first time GT_SLING_OJ=1 is used with active Witness) | Change 1 (Phase 1) |
| DetectStalePolecats marks OJ polecat as stale | HIGH | HIGH (runs every patrol cycle) | Change 5 (Phase 1) |
| POLECAT_DONE never reaches GT Witness | MEDIUM | CERTAIN (two mail systems) | Change 2 (Phase 2) |
| OJ job failure is invisible to Witness | MEDIUM | HIGH (any agent crash) | Change 4 (Phase 2) |
| Stale message detection disabled for OJ polecats | LOW | CERTAIN but safe (defaults to "allow") | Change 3 (Phase 3) |
| Name allocation leak on OJ failure | LOW | Already handled (`releasePolecatName` in sling_oj.go line 176) | None needed |

---

## 7. Open Questions

1. **Should the OJ Witness patrol (witness-patrol.hcl) coexist with the GT Witness, or should one replace the other?** Currently both can run. If GT_SLING_OJ=1 is the future, the OJ witness patrol may eventually replace the GT Witness. For now, they should coexist with the bridge in Change 2.

2. **Should OJ polecats create GT-style tmux sessions for observability?** This would make them visible to `gt polecat list` and `tmux ls`, but would conflict with OJ's ownership of the agent process. Not recommended.

3. **How should the `gt polecat nuke` command handle OJ-managed polecats?** Currently it assumes tmux. It should check for OJ job ID and delegate to `oj job cancel` first.

4. **What about hybrid rigs where some polecats are GT-managed and others are OJ-managed?** The proposed design handles this via the `oj_job_id` discriminator in the agent bead -- if present, use OJ path; if absent, use legacy tmux path.
