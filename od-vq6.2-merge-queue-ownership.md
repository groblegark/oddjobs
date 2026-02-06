# Research Spike: od-vq6.2 -- Resolve Merge Queue Ownership Between OJ and GT Refineries

## Executive Summary

Gas Town has **two independent merge queue implementations** that could theoretically process the same merge-request bead simultaneously. Today, this dual-write hazard is latent rather than active: the GT refinery is the production system, and the OJ refinery infrastructure is wired but missing its core formula. However, the OJ `merge.hcl` runbook (present in every rig's `.oj/runbooks/`) defines its own parallel merge queue that operates on raw git branches, completely bypassing the beads state layer. If both systems are ever active simultaneously on the same rig, they will corrupt each other's merges.

**Recommendation**: Adopt a **single-writer model** where GT owns the merge queue decision layer (beads MRs, scoring, lifecycle) and OJ owns the execution layer (job steps, workspace management, agent spawn). The OJ `merge.hcl` runbook should be retired and replaced by an OJ `refinery-patrol.hcl` formula that delegates to `gt refinery` commands.

---

## 1. System Map

### 1.1 GT Refinery (Go-based)

| Component | File | Role |
|-----------|------|------|
| Manager | `/home/ubuntu/gt11/gastown/internal/refinery/manager.go` | Lifecycle (start/stop tmux session), queue queries, MR operations |
| Engineer | `/home/ubuntu/gt11/gastown/internal/refinery/engineer.go` | Core merge processor: `ProcessMR`, `doDirectMerge`, `doPRMerge`, conflict delegation |
| Types | `/home/ubuntu/gt11/gastown/internal/refinery/types.go` | `MergeRequest`, `MRStatus`, state machine (open->in_progress->closed) |
| Score | `/home/ubuntu/gt11/gastown/internal/refinery/score.go` | Priority scoring: convoy age, P0-P4, retry penalty, FIFO tiebreak |
| MR ID | `/home/ubuntu/gt11/gastown/internal/mq/id.go` | `GenerateMRID()` -- SHA256-based unique IDs |
| Merge Slot | `/home/ubuntu/gt11/gastown/internal/beads/beads_merge_slot.go` | Serialized conflict resolution (acquire/release/check) |
| CLI | `/home/ubuntu/gt11/gastown/internal/cmd/refinery.go` | `gt refinery start/stop/queue/ready/blocked/claim/release` |
| Done | `/home/ubuntu/gt11/gastown/internal/cmd/done.go` | `gt done` -- creates MR bead, pushes branch, notifies witness |

### 1.2 OJ Refinery (HCL-based)

| Component | File | Role |
|-----------|------|------|
| Merge runbook | `/home/ubuntu/gt11/oddjobs/refinery/rig/.oj/runbooks/merge.hcl` | Self-contained merge queue: `oj run merge`, persisted queues, git worktree merge jobs |
| Infra queues | `/home/ubuntu/gt11/oddjobs/refinery/rig/library/gastown/infra.hcl` | Beads-backed `merge-requests` queue + `refinery` worker pointing to `refinery-patrol` job |
| Sling | `/home/ubuntu/gt11/oddjobs/refinery/rig/library/gastown/sling.hcl` | `gt-sling` command for OJ dispatch, creates MR beads in submit step |
| Polecat work | `/home/ubuntu/gt11/oddjobs/refinery/rig/library/gastown/formulas/polecat-work.hcl` | Polecat lifecycle, creates MR bead in submit step |
| Start | `/home/ubuntu/gt11/oddjobs/refinery/rig/library/gastown/start.hcl` | `gt-start` -- starts `refinery` worker via `oj worker start refinery` |
| README | `/home/ubuntu/gt11/oddjobs/refinery/rig/library/gastown/README.md` | Documents `refinery-patrol.hcl` as expected formula (line 79) |

### 1.3 GT_SLING_OJ Bridge

| Component | File | Role |
|-----------|------|------|
| OJ dispatch | `/home/ubuntu/gt11/gastown/internal/cmd/sling_oj.go` | When `GT_SLING_OJ=1`, GT allocates polecat name but delegates lifecycle to `oj run gt-sling` |

---

## 2. End-to-End Merge Flows

### 2.1 GT Flow (Production)

```
Polecat completes work
    |
    v
gt done (done.go:85-647)
    |-- Push branch to origin (line 334)
    |-- Create merge-request bead via bd.Create() (line 460-466)
    |-- Notify witness via mail (line 546)
    |-- Self-clean worktree + session
    |
    v
GT Refinery agent (Claude in tmux session gt-<rig>-refinery)
    |-- Patrol loop: calls gt refinery ready / gt refinery queue
    |-- Finds open merge-request beads via Engineer.ListReadyMRs() (engineer.go:1175-1236)
    |-- Claims MR: Engineer.ClaimMR() sets assignee (engineer.go:1330-1333)
    |-- Processes: Engineer.ProcessMRInfo() -> doMerge() (engineer.go:394-413)
    |   |-- Strategy dispatch: direct_merge | pr_to_main | pr_to_branch | direct_to_branch
    |   |-- doDirectMerge: checkout target, merge source branch, push (engineer.go:417-535)
    |   |-- doPRMerge: rebase, push branch, gh pr create (engineer.go:541-626)
    |-- On success: HandleMRInfoSuccess() (engineer.go:881-994)
    |   |-- Close MR bead, close source issue, delete branches, release merge slot
    |-- On conflict: HandleMRInfoFailure() (engineer.go:999-1039)
    |   |-- Acquire merge slot, create conflict-resolution task
    |   |-- Block MR on task via beads dependency
    |   |-- Non-blocking: queue continues to next MR
    |
    v
Work lands on target branch (or PR created)
```

**Key characteristics**:
- Source of truth: **beads merge-request wisps** queried via `bd list -t merge-request`
- Locking: **merge slot** bead for serialized conflict resolution (beads_merge_slot.go)
- Claim/release: **assignee field** on MR bead prevents double-processing
- Scoring: Deterministic priority function (score.go) -- convoy age, P0-P4, retry penalty
- Agent-driven: Claude agent makes all merge/conflict decisions (ZFC #5)

### 2.2 OJ merge.hcl Flow (Unused but Wired)

```
oj run merge <branch> <title> [--base main]
    |
    v
oj queue push merges --var branch=... --var title=... --var base=...
    |
    v
worker "merge" (concurrency=1)
    |-- Picks from persisted "merges" queue
    |-- job "merge" steps:
    |   1. init: ls-remote, fetch, create worktree on origin/<base>
    |   2. merge: git merge origin/<branch> --no-edit
    |   3. on_fail: queue-conflicts (pushes to merge-conflicts queue)
    |   4. push: retry loop (5 attempts), push local-branch:<base>, delete remote <branch>
    |   5. cleanup: remove worktree
    |
    v
worker "merge-conflict" (concurrency=1)
    |-- Picks from persisted "merge-conflicts" queue
    |-- job "merge-conflict" steps:
    |   1-2. Same init + merge attempt
    |   3. resolve: Claude agent resolves conflicts
    |   4. push: same retry loop
    |   5. cleanup
```

**Key characteristics**:
- Source of truth: **OJ persisted queue** (NOT beads)
- No beads integration: Does not create/update/close merge-request beads
- No priority scoring: FIFO queue only
- No claim mechanism: Single worker, concurrency=1
- No merge slot: No coordination with GT refinery
- Self-contained: Operates purely on git branches

### 2.3 OJ infra.hcl Refinery Worker (Aspirational)

```
infra.hcl defines:

queue "merge-requests" {
  type = "external"
  list = "bd list -t merge-request --status open --json"    <-- Reads from beads
  take = "bd update ${item.id} --status in_progress --assignee ${BD_ACTOR:-refinery}"
}

worker "refinery" {
  source      = { queue = "merge-requests" }
  handler     = { job = "refinery-patrol" }     <-- REFERENCES NON-EXISTENT JOB
  concurrency = 1
}
```

The `refinery-patrol` job is documented in the README (line 79, 96) but the file `refinery-patrol.hcl` **does not exist anywhere in the codebase**. The `gt-start` command (start.hcl:35) attempts `oj worker start refinery` which would activate this worker, but it would fail because the handler job is missing.

---

## 3. Ownership Analysis

### 3.1 Is the OJ merge.hcl Actually Used Today?

**No.** The OJ `merge.hcl` runbook is present in `.oj/runbooks/` across multiple rigs (mayor, refinery, research) but is **never invoked** by any automated flow:

1. No code calls `oj run merge` or `oj queue push merges` (grep returns zero matches).
2. The GT `gt done` command (done.go) creates beads merge-request wisps, not OJ queue items.
3. The OJ `polecat-work.hcl` submit step (line 64-69) creates a merge-request **bead** via `bd create -t merge-request`, not an OJ queue push.
4. The OJ `sling.hcl` submit step (line 107-111) also creates a merge-request **bead** via `bd create -t merge-request`.

The `merge.hcl` is an **OJ-native merge queue** that was designed as a standalone prototype. It operates on raw branches without beads and was never integrated into the Gas Town lifecycle.

### 3.2 Does the GT Refinery Run Independently of OJ?

**Yes.** The GT refinery is completely self-contained:

1. `gt refinery start` (manager.go:109-215) spawns a Claude agent in a tmux session.
2. The agent's patrol loop queries beads directly via `gt refinery ready` / `gt mq list`.
3. The Engineer (engineer.go) performs merges using the Go `git` package.
4. No OJ commands are invoked during GT refinery operation.

The only connection point is `GT_SLING_OJ=1` which delegates **polecat dispatch** to OJ, but the resulting MR bead creation still goes through beads (the OJ sling.hcl submit step creates a bead, not an OJ queue item).

### 3.3 What Locking/Coordination Exists?

| Mechanism | Scope | Implementation |
|-----------|-------|----------------|
| **Merge slot** (beads) | GT only | `beads_merge_slot.go` -- acquire/release for serialized conflict resolution |
| **MR assignee** (beads) | GT only | `ClaimMR()` sets assignee field, `ListReadyMRs()` skips assigned MRs |
| **Worker concurrency** (OJ) | OJ only | `concurrency = 1` on both `merge` and `merge-conflict` workers |
| **Cross-system** | NONE | No coordination between GT and OJ merge processors |

### 3.4 Can Both Try to Process the Same MR Bead Simultaneously?

**Theoretically yes, but not today.** The conditions for a dual-write hazard:

1. GT refinery is running (tmux session `gt-<rig>-refinery`).
2. OJ `refinery` worker is started AND has a valid `refinery-patrol.hcl` handler.
3. Both poll beads for `merge-request` wisps.
4. Both attempt to claim/process the same MR.

Today, condition #2 is not met because `refinery-patrol.hcl` does not exist. But if it were created, the hazard would be real because:

- GT claims via `assignee` field (atomic beads update).
- OJ claims via `take` command: `bd update ${item.id} --status in_progress --assignee refinery`.
- These are **separate beads commands** with no transactional guarantee.
- Race window: Both could read the MR as `open` + `no assignee` before either's claim lands.

The `merge.hcl` runbook is a separate hazard: it operates on its own OJ persisted queue, completely orthogonal to beads. If someone manually ran `oj run merge polecat/foo "some title"` while GT refinery was also processing that branch, both would attempt `git push origin main` -- a classic merge corruption scenario.

---

## 4. The Two Merge Queues Problem

There are actually **three** merge queue mechanisms in the codebase:

| Queue | Source | Consumer | State Layer | Status |
|-------|--------|----------|-------------|--------|
| **Beads MR queue** | `bd list -t merge-request` | GT Engineer | Beads (persistent) | **Production** |
| **OJ persisted queue** (merge.hcl) | `oj queue push merges` | OJ `merge` worker | OJ daemon (in-memory/disk) | **Unused** |
| **OJ external queue** (infra.hcl) | `bd list -t merge-request` | OJ `refinery` worker | Beads (same as GT!) | **Broken** (missing handler) |

Queue #3 is the dangerous one -- it reads from the **same beads source** as GT but would process via a different code path. Queue #2 is isolated but duplicative.

---

## 5. Implementation Plan

### 5.1 Recommended Ownership Model: Single-Writer with Execution Delegation

```
                    Beads (State Layer)
                         |
                    GT Refinery (Decision Layer)
                    - MR lifecycle (open -> in_progress -> closed)
                    - Priority scoring
                    - Claim/release
                    - Merge slot coordination
                    - Strategy dispatch
                         |
                    +-----------+
                    |           |
              GT Direct      OJ Execution
              (current)      (future)
              - git merge    - job steps
              - git push     - workspace mgmt
              - branch       - crash recovery
                cleanup      - agent spawn
```

**Principle**: GT owns the merge queue state machine. OJ is an optional execution backend for the physical merge steps.

### 5.2 Phase 1: Retire merge.hcl (Immediate, Low Risk)

**Goal**: Eliminate the unused OJ-native merge queue to prevent accidental activation.

**Files to modify**:
- `/home/ubuntu/gt11/oddjobs/refinery/rig/.oj/runbooks/merge.hcl` -- DELETE
- `/home/ubuntu/gt11/oddjobs/mayor/rig/.oj/runbooks/merge.hcl` -- DELETE
- `/home/ubuntu/gt11/oddjobs/crew/research/.oj/runbooks/merge.hcl` -- DELETE

**Rationale**: The `merge.hcl` runbook is never called by any automation. Its `oj run merge` command operates on raw branches, bypassing beads entirely. Leaving it in place creates a foot-gun where someone could manually invoke it and corrupt the merge state.

### 5.3 Phase 2: Disable OJ Refinery Worker in infra.hcl (Immediate, Low Risk)

**Goal**: Prevent the OJ `refinery` worker from competing with GT's refinery.

**Files to modify**:
- `/home/ubuntu/gt11/oddjobs/refinery/rig/library/gastown/infra.hcl` (lines 14-24)

**Change**: Comment out or remove the `merge-requests` queue and `refinery` worker definitions until Phase 4 provides a proper `refinery-patrol.hcl` that delegates to GT.

```hcl
# DISABLED: GT refinery owns merge queue processing.
# OJ refinery worker will be re-enabled when refinery-patrol.hcl
# is implemented as a delegation wrapper around gt refinery commands.
#
# queue "merge-requests" { ... }
# worker "refinery" { ... }
```

### 5.4 Phase 3: Add GT Refinery Claim Guard (Medium Risk)

**Goal**: Make GT refinery resilient to external claim attempts.

**Files to modify**:
- `/home/ubuntu/gt11/gastown/internal/refinery/engineer.go`

**Changes**:
1. In `ClaimMR()` (line 1330), add an atomic check-and-set: read current assignee, verify empty, then update. This prevents the TOCTOU race if OJ infra ever becomes active.

2. In `ListReadyMRs()` (line 1175), add a staleness check: if an MR has been `in_progress` with a specific assignee for more than N minutes, consider it stale-claimed and eligible for re-claim. This addresses crash recovery.

### 5.5 Phase 4: Create refinery-patrol.hcl as GT Delegation Wrapper (Medium Risk)

**Goal**: Create the missing `refinery-patrol.hcl` so OJ can serve as a monitoring/restart layer for the GT refinery, without owning merge logic.

**New file**: `/home/ubuntu/gt11/oddjobs/refinery/rig/library/gastown/formulas/refinery-patrol.hcl`

**Design**: The OJ refinery-patrol job would:
1. Check if GT refinery tmux session is alive (health monitoring).
2. If dead, restart via `gt refinery start <rig>`.
3. Check beads for stale `in_progress` MRs (crash recovery).
4. Report status via OJ notifications.

This keeps OJ in its natural role (execution monitoring, crash recovery) while GT retains merge decision authority.

```hcl
# Formula: Refinery Patrol -- GT Refinery Health Monitor
#
# This is NOT a merge queue processor. It monitors the GT refinery
# and restarts it if it dies. Merge decisions stay with GT.

job "refinery-patrol" {
  name = "refinery-patrol"
  workspace = "folder"

  step "check-health" {
    run = <<-SHELL
      # Check if GT refinery session is alive
      SESSION="gt-${GT_RIG:-default}-refinery"
      if tmux has-session -t "$SESSION" 2>/dev/null; then
        echo "GT refinery is alive: $SESSION"
        # Check for stale MRs (in_progress > 30min without progress)
        bd list -t merge-request --status in_progress --json 2>/dev/null \
          | jq -r '.[] | select(.updated_at < (now - 1800 | todate)) | .id' \
          | while read MR_ID; do
              echo "WARNING: Stale MR detected: $MR_ID"
              bd update "$MR_ID" --status open --assignee "" 2>/dev/null || true
            done
      else
        echo "GT refinery is dead, restarting..."
        gt refinery start "${GT_RIG:-default}" 2>/dev/null || true
      fi
    SHELL
  }
}
```

### 5.6 Phase 5: Re-enable OJ Refinery Worker with Delegation (Low Risk)

After Phase 4, re-enable the OJ worker in `infra.hcl` pointing to the new delegation-based `refinery-patrol` job. Update the queue definition to be a monitoring-only poll, not a processing trigger.

### 5.7 Phase 6: Document the Ownership Contract (Low Risk)

Add a clear ownership comment to all relevant files:

**Files to update**:
- `/home/ubuntu/gt11/oddjobs/refinery/rig/library/gastown/infra.hcl` -- Header comment
- `/home/ubuntu/gt11/oddjobs/refinery/rig/library/gastown/README.md` -- Architecture section
- `/home/ubuntu/gt11/gastown/refinery/rig/docs/concepts/refinery-merge-workflow.md` -- Add OJ section
- `/home/ubuntu/gt11/gastown/internal/refinery/engineer.go` -- Doc comment on Engineer struct

---

## 6. Locking/Coordination Protocol Design

### 6.1 Current State

GT has two levels of coordination:
1. **MR assignee** (soft lock): `ClaimMR()` sets assignee, `ListReadyMRs()` skips assigned. No CAS guarantee.
2. **Merge slot** (hard lock): Single bead-backed semaphore for conflict resolution serialization.

OJ has:
1. **Worker concurrency=1** (process-level): Only one merge job runs at a time per OJ daemon.

### 6.2 Proposed Protocol

Since the recommendation is single-writer (GT owns merge decisions), the protocol simplifies to:

1. **GT Refinery is the sole writer** for MR bead status transitions.
2. **OJ observes only**: The OJ refinery-patrol job reads MR status but never writes `status` or `assignee`.
3. **Health monitoring is idempotent**: OJ can safely restart the GT refinery tmux session because `Manager.Start()` (manager.go:119-124) already checks for existing sessions and returns `ErrAlreadyRunning`.
4. **Stale claim recovery**: OJ patrol can reset stale claims (Phase 4) because the claim is a soft lock with no write-ahead log. Resetting a stale claim is always safe -- the original claimant either crashed (recovery is correct) or is still running (it will re-claim on next poll).

### 6.3 Future: If OJ Needs to Own Merge Execution

If a future phase moves merge execution to OJ (e.g., for workspace management benefits), the protocol would need:

1. **Beads CAS operation**: `bd update --if-assignee="" --assignee=<worker>` to prevent TOCTOU races.
2. **Heartbeat on in_progress**: The processing worker writes a `last_heartbeat` field periodically. Stale claims are detected by comparing `last_heartbeat` to current time.
3. **Merge slot delegation**: GT Engineer's merge slot logic would need to be accessible from OJ (either via `gt refinery merge-slot acquire` CLI or a beads-native slot protocol).

---

## 7. Dependencies on Other od-vq6 Subtasks

| Subtask | Dependency | Nature |
|---------|------------|--------|
| od-vq6.1 (Sling dispatch) | GT_SLING_OJ bridge | If sling dispatch moves to OJ, MR bead creation must remain in beads (not OJ queue). The current OJ sling.hcl correctly does `bd create -t merge-request`. |
| od-vq6.3 (Polecat lifecycle) | Self-cleaning model | GT done's self-nuke happens before refinery processes. No conflict, but OJ polecat lifecycle must also create beads MRs (not OJ queue items). |
| od-vq6.4 (Witness integration) | MERGED mail | Both GT refinery and OJ refinery would need to send MERGED mail. With single-writer model, only GT sends mail. |
| od-vq6.5 (Daemon/heartbeat) | Boot triage | OJ boot-triage (boot-triage.hcl) already monitors merge queue length. It must not attempt to restart merge processing independently. |

---

## 8. Migration Path

### Immediate (Phase 1-2): De-risk

1. Delete all `merge.hcl` runbook copies from `.oj/runbooks/`.
2. Comment out `merge-requests` queue and `refinery` worker in `infra.hcl`.
3. No behavioral change -- GT refinery continues as sole processor.

### Short-term (Phase 3-4): Harden

1. Add claim guard to GT Engineer.
2. Create `refinery-patrol.hcl` as health monitor.
3. GT refinery gains crash recovery via OJ monitoring.

### Medium-term (Phase 5-6): Integrate

1. Re-enable OJ worker with delegation handler.
2. Document ownership contract.
3. System has clean separation: GT=decisions, OJ=monitoring.

### Long-term (Optional): Execution Migration

If OJ proves reliable for polecat lifecycle management (od-vq6.1/6.3), consider moving the physical merge steps (git checkout, merge, push) to OJ jobs while GT retains decision authority. This would require the CAS protocol from section 6.3.

---

## 9. Risk Assessment

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| merge.hcl accidentally invoked | Low | High (merge corruption) | Phase 1: delete the files |
| OJ refinery worker starts without handler | Low | Low (fails cleanly) | Phase 2: remove queue/worker definitions |
| Both GT and OJ process same MR | Very Low (today) | High (double merge, data corruption) | Phase 3: claim guard with CAS |
| GT refinery crashes, no one restarts | Medium | Medium (MRs pile up) | Phase 4: OJ health monitor |
| Stale claims block queue | Low | Medium (MRs stuck) | Phase 3: staleness timeout |

---

## 10. Appendix: File Index

All absolute paths referenced in this report:

**GT Refinery (Go)**:
- `/home/ubuntu/gt11/gastown/internal/refinery/manager.go` -- Lifecycle manager
- `/home/ubuntu/gt11/gastown/internal/refinery/engineer.go` -- Core merge processor
- `/home/ubuntu/gt11/gastown/internal/refinery/types.go` -- MR types and state machine
- `/home/ubuntu/gt11/gastown/internal/refinery/score.go` -- Priority scoring
- `/home/ubuntu/gt11/gastown/internal/mq/id.go` -- MR ID generation
- `/home/ubuntu/gt11/gastown/internal/beads/beads_merge_slot.go` -- Merge slot locking
- `/home/ubuntu/gt11/gastown/internal/cmd/refinery.go` -- CLI commands
- `/home/ubuntu/gt11/gastown/internal/cmd/done.go` -- gt done (MR creation)
- `/home/ubuntu/gt11/gastown/internal/cmd/sling_oj.go` -- OJ dispatch bridge

**OJ Refinery (HCL)**:
- `/home/ubuntu/gt11/oddjobs/refinery/rig/.oj/runbooks/merge.hcl` -- Standalone merge queue (UNUSED)
- `/home/ubuntu/gt11/oddjobs/mayor/rig/.oj/runbooks/merge.hcl` -- Same (copy)
- `/home/ubuntu/gt11/oddjobs/crew/research/.oj/runbooks/merge.hcl` -- Same (copy)
- `/home/ubuntu/gt11/oddjobs/refinery/rig/library/gastown/infra.hcl` -- Shared queues/workers
- `/home/ubuntu/gt11/oddjobs/refinery/rig/library/gastown/sling.hcl` -- OJ sling dispatch
- `/home/ubuntu/gt11/oddjobs/refinery/rig/library/gastown/formulas/polecat-work.hcl` -- Polecat lifecycle
- `/home/ubuntu/gt11/oddjobs/refinery/rig/library/gastown/start.hcl` -- Town startup
- `/home/ubuntu/gt11/oddjobs/refinery/rig/library/gastown/formulas/boot-triage.hcl` -- Watchdog
- `/home/ubuntu/gt11/oddjobs/refinery/rig/library/gastown/README.md` -- OJ GT architecture docs

**Design Docs**:
- `/home/ubuntu/gt11/gastown/refinery/rig/docs/design/merge-queue-strategies.md` -- Strategy design
- `/home/ubuntu/gt11/gastown/refinery/rig/docs/concepts/refinery-merge-workflow.md` -- Workflow docs
- `/home/ubuntu/gt11/gastown/refinery/rig/docs/design/architecture.md` -- Architecture overview
