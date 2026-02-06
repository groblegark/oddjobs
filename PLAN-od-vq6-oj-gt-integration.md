# OJ-GT Integration Hardening Plan

**Epic:** od-vq6 | **Date:** 2026-02-06 | **Author:** oddjobs/crew/research

---

## Overview

Gas Town runs two converging systems:
- **Gastown (GT)** -- Go CLI/RPC orchestrator (67 packages). Human-facing: work dispatch, mail, advice, merge queue.
- **Oddjobs (OJ)** -- Rust job engine (8 crates). Deterministic: runbooks, queues, workers, steps.

The bridge is `sling_oj.go` + `library/gastown/sling.hcl`. When `GT_SLING_OJ=1`, GT delegates polecat lifecycle to OJ while retaining bead state management.

This plan addresses 6 hardening tasks identified via deep research spikes across both codebases.

---

## Priority Order

| # | Bead | Title | Severity | Effort |
|---|------|-------|----------|--------|
| 1 | od-vq6.1 | Witness handoff protocol | **CRITICAL** (data loss) | Medium |
| 2 | od-vq6.2 | Merge queue ownership | **HIGH** (corruption risk) | Low-Medium |
| 3 | od-vq6.4 | Auto-close beads on merge | Medium | Low |
| 4 | od-vq6.3 | Runbook version headers | Medium | Low |
| 5 | od-vq6.5 | Integration test suite | Medium | Medium-High |
| 6 | od-vq6.6 | Gitignore consistency | Low | Low |

---

## 1. Witness Handoff Protocol (od-vq6.1) -- CRITICAL

### Problem

The GT Witness uses tmux session existence as source of truth for polecat liveness. OJ-managed polecats have no tmux session. **The Witness WILL nuke OJ-managed polecats** -- `AutoNukeIfClean()` in `handlers.go:761` checks `cleanup_status`, and if "clean", calls `NukePolecat()` at line 716. This deletes the worktree out from under a running OJ job.

Additionally, OJ sends POLECAT_DONE via `bd create -t message` (beads database), while GT Witness reads from `mail.NewRouter` (file-based mailboxes). These are **two separate mail systems** that don't see each other.

### Discriminator

The `oj_job_id` field, already stored in beads by `storeOjJobIDInBead()` (`sling_oj.go:137`), serves as the discriminator. Present = OJ-managed. Absent = legacy tmux path.

### Plan

**Phase 1 -- Prevent data loss (immediate):**

| Change | File | Description |
|--------|------|-------------|
| Guard AutoNukeIfClean | `gastown/internal/witness/handlers.go:761` | Before nuking, check agent bead for `oj_job_id`. If present, query `oj job show <id>` instead of tmux. Only nuke if OJ job is terminal (completed/failed). |
| Guard DetectStalePolecats | `gastown/internal/polecat/manager.go` | Skip OJ-managed polecats in stale detection. Check `oj_job_id` in agent bead description. |

**Phase 2 -- Enable monitoring (short-term):**

| Change | File | Description |
|--------|------|-------------|
| Bridge beads mail | `gastown/internal/witness/handlers.go` | Add `PollBeadsInbox()` function: query `bd list -t message --label to:witness --status open` alongside existing file-based mailbox. |
| OJ failure notification | `library/gastown/sling.hcl` (reopen step) | Add `bd create -t message --labels "to:witness,msg-type:polecat-failed"` so Witness learns about OJ-side failures. |

**Phase 3 -- Documentation:**

| Change | File | Description |
|--------|------|-------------|
| State machine diagram | `docs/concepts/oj-witness-handoff.md` | Text-based state machine showing who monitors at each step. |

### State Machine (OJ Path)

```
gt sling (GT_SLING_OJ=1)
    |
    v
[GT: AllocateName, create agent bead with oj_job_id]
    |
    v
[OJ: oj run gt-sling --> provision --> execute --> submit]
    |                                                |
    |   (Witness sees oj_job_id, queries OJ)         |
    |   (if OJ job running --> skip, don't nuke)     |
    v                                                v
[OJ: cleanup step]                    [OJ: sends POLECAT_DONE via beads mail]
    |                                                |
    v                                                v
[OJ: job completes]                   [Witness: PollBeadsInbox picks up message]
    |                                                |
    v                                                v
[Witness: next patrol sees oj_job_id   [Witness: processes POLECAT_DONE normally]
  + OJ job terminal --> safe to clean]
```

### Dependencies
- None (self-contained in gastown rig)

---

## 2. Merge Queue Ownership (od-vq6.2) -- HIGH

### Problem

Three merge queue mechanisms exist:
1. **Beads MR queue (GT)** -- Production system. `engineer.go` processes MRs with priority scoring, claim/release, merge slot coordination.
2. **OJ persisted queue (`merge.hcl`)** -- Standalone, operates on raw git branches bypassing beads entirely. Present in `.oj/runbooks/` but never invoked by automation.
3. **OJ external queue (`infra.hcl`)** -- Reads same beads source as GT, but its handler job `refinery-patrol` **does not exist**.

The OJ `merge.hcl` is the most dangerous artifact. If someone runs `oj run merge <branch> <title>` while GT refinery is active, both push to main and corrupt each other.

### Plan

**Phase 1 -- Flag and disable in gastown, preserve for legacy rigs (immediate):**

| Change | File | Description |
|--------|------|-------------|
| Add feature flag | `library/gastown/infra.hcl` | Add `OJ_LEGACY_MERGE=1` env var check. When unset (gastown default), disable the OJ merge queue and refinery worker. When set, legacy rigs can still use OJ-native merge. |
| Guard merge.hcl | `oddjobs/*/rig/.oj/runbooks/merge.hcl` | Add header comment: "LEGACY -- only for non-gastown rigs. In gastown, GT refinery owns merge decisions." Do NOT delete -- other rigs outside gastown may depend on it. |
| Disable OJ merge worker in gastown | `library/gastown/infra.hcl` | Comment out the `merge-requests` queue and `refinery` worker blocks for gastown. Retain the definitions gated behind `OJ_LEGACY_MERGE` for legacy use. |

**Phase 2 -- Establish single-writer (short-term):**

| Change | File | Description |
|--------|------|-------------|
| Add claim guard | `gastown/internal/refinery/engineer.go` | Add CAS semantics to MR claim: `bd update <mr> --status in_progress --expect-status open`. Prevents double-processing. |
| Create refinery-patrol.hcl | `library/gastown/formulas/refinery-patrol.hcl` | OJ health-monitor formula that delegates to GT: restarts crashed GT refinery, resets stale claims, reports queue depth. |

**Phase 3 -- Re-enable OJ monitoring (medium-term):**

| Change | File | Description |
|--------|------|-------------|
| Re-enable OJ worker | `library/gastown/infra.hcl` | Point `refinery` worker at new `refinery-patrol` job (delegation, not direct merging). |
| Document ownership | `library/gastown/README.md` | Add "Merge Queue Ownership" section: GT = decision layer, OJ = monitoring layer. |

### Ownership Model

```
GT (single writer):           OJ (optional monitor):
  Claim MR bead                 Health-check GT refinery
  Rebase + test                 Restart if crashed
  Push to main                  Reset stale claims
  Close MR + source bead        Report queue depth
  Send MERGED mail              Escalate stuck merges
```

### Dependencies
- od-vq6.4 (bead auto-close) should coordinate on MR bead lifecycle

---

## 3. Auto-Close Beads on Merge (od-vq6.4)

### Problem

The GT refinery already auto-closes source beads post-merge (`engineer.go:808-820`, `beads.CloseWithReason()`). The problem is exclusively in the OJ path:
- `bug.hcl:64` calls `wok done ${var.bug.id}` **before** the merge, not after
- The merge job receives only `branch` and `title` vars -- no `issue_id`
- If the merge fails, the bead is incorrectly marked done

### Plan

| # | Change | File | Description |
|---|--------|------|-------------|
| 1 | Add `issue_id` var to merge queue | `.oj/runbooks/merge.hcl:25,31` | `vars = ["branch", "title", "base", "issue_id"]` with default `""` |
| 2 | Add post-merge close step | `.oj/runbooks/merge.hcl` | New `close-issue` step between `push` and `cleanup`: `wok done "${var.mr.issue_id}"` (guarded by `if [ -n ... ]`) |
| 3 | Thread issue_id from submit | `.oj/runbooks/bug.hcl:65` | Add `--var issue_id="${var.bug.id}"` to `oj queue push merges` |
| 4 | Remove premature close | `.oj/runbooks/bug.hcl:64` | Delete `wok done ${var.bug.id}` (and same in `chore.hcl:64`) |
| 5 | Thread through conflict path | `.oj/runbooks/merge.hcl:87` | Pass `--var issue_id="${var.mr.issue_id}"` to merge-conflicts queue |
| 6 | Remove premature close (GT) | `library/gastown/formulas/polecat-work.hcl:77` | Remove `bd close` (GT refinery handles post-merge close already) |

### Edge Cases
- **Merge failure**: Issue stays open (correct -- merge didn't land)
- **Manual merge without issue**: `issue_id` defaults to empty, close-issue is a no-op
- **Reverts**: Already-closed bead stays closed. New issue filed for regression.
- **Conflict resolution**: `issue_id` threaded through conflict queue, same close-issue step fires after resolution

### Dependencies
- od-vq6.2 (merge queue ownership) -- if merge.hcl is deleted, these changes apply to its replacement or to the library version only

---

## 4. Runbook Version Headers (od-vq6.3)

### Problem

`ensureOjRunbook()` in `sling_oj.go:200-230` copies `library/gastown/sling.hcl` to `.oj/runbooks/gt-sling.hcl` but **only if the target doesn't exist**. If the source changes, the stale copy is used indefinitely.

### Constraint

OJ's Rust parser uses `#[serde(deny_unknown_fields)]` on the `Runbook` struct (`parser.rs:68`). Adding a new HCL block would cause a parse error. Version info must live in **comments**.

### Format Specification

```hcl
# @gt-library: library/gastown/sling.hcl
# @gt-version: 1
#
# Sling -- OJ-Managed Work Dispatch
# ...existing comments...
```

- `@gt-library`: Source path (identifies managed copies vs user-authored files)
- `@gt-version`: Monotonically increasing integer (bumped by library author)
- Files without headers treated as version 0 (unversioned)

### Plan

| # | Change | File | Complexity |
|---|--------|------|------------|
| 1 | Add version headers | `library/gastown/sling.hcl` | Trivial |
| 2 | Add `extractGtVersion()` helper | `gastown/internal/cmd/sling_oj.go` | ~20 lines Go |
| 3 | Update `ensureOjRunbook()` | `gastown/internal/cmd/sling_oj.go` | ~40 lines Go -- compare versions, auto-update if stale |
| 4 | Add `gt doctor` check | `gastown/internal/doctor/oj_runbook_check.go` (new) | ~80 lines Go -- FixableCheck that copies source to dest |
| 5 | Register doctor check | `gastown/internal/cmd/doctor.go:~162` | 1 line |

### Mismatch Policy

| Scenario | Action |
|----------|--------|
| Dest missing | Copy. Log "Installed gt-sling.hcl (version N)" |
| Dest version < source | Overwrite. Log "Updated (version X -> Y)" |
| Dest version == source | No-op |
| No version headers | Fall back to content hash comparison |

### Dependencies
- None (self-contained)

---

## 5. Integration Test Suite (od-vq6.5)

### Problem

**Zero tests exercise the OJ dispatch path.** `sling_oj.go` has 6 functions (255 lines) with no unit tests. No file in the codebase sets `GT_SLING_OJ=1` in a test context.

### Three-Layer Design

#### Layer 1: GT Unit Tests (`sling_oj_test.go`)

| Test | What It Covers |
|------|----------------|
| `TestParseOjJobID` (6 subtests) | JSON format, "Job started:" format, "job_id:" format, single-line fallback, empty, multiline |
| `TestEnsureOjRunbook` (3 subtests) | Copy from library, skip if exists, fail if source missing |
| `TestGetBeadBase` (2 subtests) | Default to main, read from label |
| `TestOjSlingEnabled` (3 subtests) | Env set/unset/0 |
| `TestDispatchToOj` (2 subtests) | Happy path with oj stub, failure with name release |

Infrastructure: `writeOjStub(t, binDir, script)` helper mirroring existing `writeBDStub` pattern.

#### Layer 2: OJ Runbook Specs (`specs/gastown/sling_runbook.rs`)

| Test | What It Covers |
|------|----------------|
| `gt_sling_runbook_parses` | HCL file loads without errors |
| `gt_sling_provision_step_runs` | With bd/gt/git stubs |
| `gt_sling_submit_step_runs` | MR bead created, mail sent |
| `gt_sling_cleanup_removes_worktree` | Git worktree removal |
| `gt_sling_reopen_on_failure` | Failure -> reopen flow |

Infrastructure: Stub scripts for `bd`, `gt`, `git` in test PATH.

#### Layer 3: Cross-System E2E (`tests/e2e/gt_sling_oj.sh`)

Full path: `GT_SLING_OJ=1 gt sling` -> `oj run gt-sling` -> all 4 steps -> verify bead state. Following existing `merge_queue.sh` pattern.

### Plan

| Phase | Deliverable | Effort |
|-------|-------------|--------|
| Phase 1 | `sling_oj_test.go` (13 tests) | 1 session |
| Phase 2 | `specs/gastown/sling_runbook.rs` (5 tests) | 1-2 sessions |
| Phase 3 | `tests/e2e/gt_sling_oj.sh` (1 E2E) | 1-2 sessions |
| Phase 4 | CI integration | 0.5 sessions |

### Dependencies
- od-vq6.3 (runbook format must be finalized before Layer 2)
- od-vq6.1 (witness behavior must be finalized before E2E)

---

## 6. Gitignore Consistency (od-vq6.6)

### Problem

- **OJ `.gitignore`**: Only 2 lines (`plugins/`, `.repo.git/`). Runtime artifacts leak into `git status`.
- **GT `.gitignore`**: Comprehensive (binaries, runtime, beads, IDE, OS files).
- **Generated `gt-sling.hcl`**: Not in any gitignore, shows as untracked.
- **GT provisions crew gitignore**: `gt` binary injects a "Gas Town (added by gt)" section at workspace creation. The OJ crew `.gitignore` has this section but the upstream OJ `.gitignore` doesn't.

### Current Untracked Files in OJ

```
.oj/runbooks/gt-sling.hcl    # Generated by sling_oj.go
FILE_AFTER_FAIL.md            # Agent-generated
state.json                    # Runtime state
```

### Plan

| # | Change | File | Description |
|---|--------|------|-------------|
| 1 | Add OJ-specific ignores | `oddjobs/.gitignore` | Add: `.oj/runbooks/gt-*.hcl`, `state.json`, `*/FILE_*.md`, `.runtime/`, `.claude/`, `CLAUDE.md` |
| 2 | Add beads runtime ignores | `oddjobs/.gitignore` | Mirror GT's `.beads/` patterns: `*.db`, `daemon.*`, `*.sock`, `redirect`, `last-touched` |
| 3 | Verify GT injection | Check crew/research `.gitignore` | Confirm "Gas Town (added by gt)" section covers runtime files |

### Dependencies
- None (self-contained)

---

## Implementation Sequence

```
Week 1 (CRITICAL):
  [1] od-vq6.1 Phase 1 -- Guard witness against OJ polecats (prevents data loss)
  [2] od-vq6.2 Phase 1 -- Flag and disable merge.hcl in gastown (prevents corruption risk)

Week 2 (HIGH):
  [3] od-vq6.4 -- Thread issue_id through merge queue (correct bead lifecycle)
  [4] od-vq6.3 -- Add version headers + auto-update (prevent stale runbooks)
  [5] od-vq6.6 -- Fix gitignore (quality of life)

Week 3 (MEDIUM):
  [6] od-vq6.5 Phase 1 -- GT unit tests for sling_oj.go
  [7] od-vq6.1 Phase 2 -- Bridge beads mail to witness
  [8] od-vq6.2 Phase 2 -- Claim guard + refinery-patrol.hcl

Week 4+ (ONGOING):
  [9] od-vq6.5 Phase 2-3 -- OJ specs + E2E tests
  [10] od-vq6.1 Phase 3 + od-vq6.2 Phase 3 -- Documentation
```

---

## Cross-Cutting Concerns

### Rig Routing
Changes to OJ runbooks must be applied in **3 copies** (refinery/rig, mayor/rig, crew/research) until a unified runbook registry is established.

### Testing
Every change should be validated against both the GT and OJ paths. The integration test suite (od-vq6.5) provides the long-term safety net, but manual testing is required for Phase 1 critical fixes.

### Backward Compatibility
All changes maintain backward compatibility with `GT_SLING_OJ=0` (legacy tmux path). The `oj_job_id` discriminator ensures hybrid operation where some polecats are GT-managed and others are OJ-managed.

---

## Detailed Research Reports

Full spike reports are available at:
- `od-vq6.1-witness-handoff-protocol.md` (witness handoff)
- `od-vq6.2-merge-queue-ownership.md` (merge queue ownership)
- Agent transcripts for od-vq6.3, od-vq6.4, od-vq6.5, od-vq6.6 (in session context)

## Beads

All work is tracked under epic `od-vq6` with children:
- `od-vq6.1`: Document witness handoff protocol
- `od-vq6.2`: Resolve merge queue ownership
- `od-vq6.3`: Add runbook version headers + mismatch detection
- `od-vq6.4`: Auto-close beads on merge landing
- `od-vq6.5`: Cross-system integration test suite
- `od-vq6.6`: Gitignore inconsistency fix
