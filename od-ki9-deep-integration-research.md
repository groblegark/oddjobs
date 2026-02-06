# OJ-GT Deep Integration Research

**Epic:** od-ki9 | **Date:** 2026-02-06 | **Author:** oddjobs/crew/research

---

## Context

This document extends the convergence analysis (GT_OJ_CONVERGENCE_ANALYSIS.md) and
hardening plan (PLAN-od-vq6-oj-gt-integration.md) with deeper integration proposals.

Five research agents explored OJ internals across runbooks, daemon/worker architecture,
agent adapters, queue/event systems, and state/persistence. This document distills
their findings into actionable integration paths, scored by feasibility and value.

**Core principle:** GT owns orchestration and state (the what). OJ owns execution and
monitoring (the how). **Beads are the data plane.**

---

## Integration Proposals

### Proposal A: Beads as OJ's Native State Layer [RECOMMENDED]

**Status:** Easy win. High value, clear path.

**Problem:** OJ maintains its own WAL + snapshot persistence (MaterializedState) which
duplicates state already tracked in GT beads. OJ jobs, their step transitions, agent
assignments, and decisions all exist in a parallel universe invisible to GT unless
explicitly bridged.

**Proposal:** Make GT beads the canonical state store for OJ job lifecycle.

**Mapping:**

| OJ Concept | Bead Representation |
|---|---|
| Job created | Task bead created with `oj:job` label |
| Step transition | Bead label update (`oj:step:<name>`), status change |
| Agent assigned | Agent bead's `hook_bead` points to job bead |
| Decision point | DECISION-type bead, job bead DEPENDS_ON it |
| Job failed | Bead status BLOCKED + `oj:failed` label |
| Job completed | Bead status CLOSED |
| Queue item | Bead with appropriate type (merge-request, bug, task) |

**What this gives us:**

1. **Single source of truth** -- no syncing between OJ WAL and beads
2. **Dolt audit trail** -- every job state change is a versioned commit
3. **Cross-system visibility** -- GT users see OJ jobs as beads in `bd list`
4. **Unified queries** -- `bd list -l oj:job --status open` shows all running OJ work
5. **Crash recovery via Dolt** -- reconstruct OJ state from bead history

**What OJ WAL still does:**

OJ's WAL remains for its own internal consistency (effect execution, timer state,
session ephemera). The proposal is not to *replace* the WAL but to make beads the
*authoritative* state for job lifecycle, with OJ syncing to beads on every transition.

**Implementation sketch:**

```
OJ Event (JobCreated/JobAdvanced/etc.)
  → Runtime handler fires
  → Existing WAL write (unchanged)
  → NEW: bd CLI call to create/update corresponding bead
  → Bead state becomes queryable by GT, witness, deacon
```

**Key files:**
- `crates/storage/src/state.rs:256` -- `MaterializedState::apply_event()` hook point
- `crates/core/src/event.rs:78` -- Event enum for mapping
- `library/gastown/infra.hcl` -- external queues already poll beads

**Open question:** Should OJ shell out to `bd` CLI or link beads as a Rust library?
CLI is simpler and decoupled; library is faster but creates a build dependency.

**Effort:** Medium (2-3 weeks). Most work is in the event-to-bead mapping layer.

---

### Proposal B: Bidirectional Reactive Event Bus [RESEARCH NEEDED]

**Status:** High value. Needs queue unification research.

**Problem:** GT and OJ communicate through polling. GT's witness polls `witness-inbox`
via `bd list`. OJ workers poll external queues via shell commands. Polling introduces
latency (30s-5m) and missed state. Neither system reacts to the other's events in
real-time.

**Proposal:** A lightweight event bridge that:

1. **OJ → GT:** Transforms OJ WAL events into GT `.events.jsonl` entries. OJ job
   milestones (started, step advanced, agent idle, completed) appear in GT's feed.

2. **GT → OJ:** Listens to `bd activity --follow` and routes bead events into OJ
   queue pushes or decision triggers. When a merge-request bead is created, it's
   immediately pushed to OJ's refinery queue (no polling delay).

3. **Declarative rules:** Reactive subscriptions defined in HCL/YAML:
   ```
   trigger: bead_created { type = "merge-request" }
   action:  oj_queue_push { queue = "merge-requests" }
   ```

**What this gives us:**

1. Sub-second event coupling (replace polling with push)
2. Unified observability (single event stream for both systems)
3. Extensible (new rules without code changes)
4. WAL-backed (crash-recoverable event replay)

**Relationship to Proposal A:** If beads are the state layer (Proposal A), then the
event bus becomes the *notification* layer. Beads hold truth; events signal changes.
These two proposals are complementary.

**Queue unification angle:** Currently OJ has two queue types:
- **Persisted queues:** WAL-backed, daemon state
- **External queues:** Shell command polling (`bd list`, `wok ready`, `gh pr list`)

If beads become the canonical state (Proposal A), then external queues that already
poll beads become redundant -- the event bus can push directly. Persisted queues could
be backed by beads instead of WAL, unifying all queue state into one system.

**Research result:** Beads is ALREADY the queue backend (all Gas Town queues in
infra.hcl poll `bd list`). The unification path is clear: add an event bridge
that pushes on bead creation instead of polling, then retire OJ persisted queues
in the GT context. See Appendix A2 for full analysis.

**Effort:** Medium (2-3 weeks). Event bridge crate + infra.hcl updates.

---

### Proposal C: GT Lifecycle Hooks in OJ [RESEARCH NEEDED]

**Status:** Promising. Need to understand what OJ already provides.

**Problem:** GT has no way to influence OJ's execution decisions. When OJ picks up a
queue item, GT can't reorder by priority. When a job fails, GT can't decide retry vs
escalate. GT's graph knowledge (dependencies, resource state, cost budgets) is invisible
to OJ's runtime.

**Proposal:** OJ exposes lifecycle hook points that GT can register callbacks for:

| Hook Point | GT Can... |
|---|---|
| `before_worker_poll` | Adjust concurrency based on rig load |
| `filter_items` | Reorder queue items by priority/dependency |
| `on_job_created` | Inject vars, workspace hints, time budgets |
| `on_job_failed` | Decide: retry, escalate, cancel, reroute |
| `on_job_terminal` | Trigger downstream work, update dependency graph |

**What OJ already has:**

OJ runbooks already define per-step handlers:
- `on_done` / `on_fail` / `on_cancel` -- step transition rules
- `on_idle` / `on_dead` -- agent lifecycle handlers
- `notify { on_start, on_done, on_fail }` -- notification hooks

The question is whether these existing mechanisms are sufficient for GT's needs, or
whether a new external hook API is required.

**Research result:** OJ has NO external hook mechanism. Internal handlers
(`on_done`/`on_fail`/`on_idle`/`on_dead`) are step transitions and agent state
machine actions, not external callbacks. The `notify` system is desktop-only.

**Viable approaches (ascending effort):**
1. Custom job steps that shell out to GT (no OJ changes, per-runbook config)
2. Custom `NotifyAdapter` that POSTs to GT (small OJ change, automatic for all jobs)
3. WAL event replay service (no OJ changes, higher latency)
4. New `Effect::InvokeHook` with daemon socket API (full hook system, most effort)

See Appendix A1 for full analysis.

**Key consideration:** Hooks must be non-blocking (OJ can't stall waiting for GT).
Timeout + fallback-to-default is essential.

**Effort:** Low-Medium for approach #1-2. Medium-High for approach #4.

---

### Proposal D: Dynamic Runbook Generation [DEFERRED]

**Status:** Interesting but speculative. Not actionable yet.

**Concept:** GT formulas could dynamically generate OJ runbooks at dispatch time,
parameterized by bead metadata (task complexity, prior execution metrics, rig load).
This would enable adaptive agent model selection, timeout tuning, and concurrency
adjustment.

**Why deferred:**

1. The static runbook model works. No evidence of runbook rigidity blocking real work.
2. Complexity is high -- HCL generation, validation, versioning all become GT's problem.
3. Better to stabilize the state layer (Proposal A) and hook system (Proposal C) first.
   Dynamic generation can layer on top later if needed.
4. Some benefits (adaptive timeouts, model selection) can be achieved through simpler
   means: bead advice beads that agents read at startup, or hook-injected vars.

**Revisit when:** Proposals A and C are implemented and we have data on what parameters
actually need runtime tuning.

---

### Proposal E: Shared Agent Context / Memory [NOT NEEDED]

**Status:** Rejected in favor of existing mechanisms.

**Concept:** A SharedAgentContext object persisted in beads that carries findings,
genealogy, and directives across agent boundaries.

**Why not needed:**

OJ workers and GT polecats already interact through beads work items and mail. The
existing pattern is:
- Agent writes findings to bead notes/comments
- Next agent reads bead notes/comments
- Mail system (witness-inbox) handles lifecycle notifications
- Molecule steps (child beads) track work decomposition

This is sufficient for cross-agent context sharing. A dedicated SharedAgentContext
abstraction would add complexity without clear benefit over the beads-native approach.

**What we should do instead:** Ensure the bead notes/comments pattern is well-documented
and that OJ agents reliably write structured output to beads (not just to local files).

---

## Priority Order

| # | Proposal | Value | Effort | Status |
|---|----------|-------|--------|--------|
| 1 | **A: Beads as OJ State** | High | Medium | Ready to implement |
| 2 | **B: Reactive Event Bus** | High | Medium-High | Research pending (queues) |
| 3 | **C: GT Lifecycle Hooks** | Medium-High | TBD | Research pending (OJ hooks) |
| 4 | D: Dynamic Runbooks | Medium | High | Deferred |
| 5 | E: Shared Agent Context | Low | Medium | Not needed |

---

## Migration Path: Incremental Integration with Validation Workflows

Each phase produces a **functioning workflow** that validates assumptions before
advancing. If a phase reveals bad assumptions, we course-correct before investing
in the next layer.

### Phase 0: Clean Coexistence (od-vq6) [NOW]

**Goal:** Ensure OJ and GT coexist cleanly. No new architecture, just proper
boundaries. (Note: no active data loss today, but these are good hygiene.)

**Deliverables:**
- Witness checks `oj_job_id` before acting on polecats (od-vq6.1)
- OJ merge.hcl gated behind `OJ_LEGACY_MERGE=1` (od-vq6.2)
- `--engine=oj|gt` flag replaces `GT_SLING_OJ` env var

**Validation workflow:** Sling a polecat via `gt sling --engine=oj`, verify witness
handles it correctly, verify merge goes through GT refinery not OJ merge.hcl.

**Assumption tested:** OJ and GT can coexist cleanly with minimal changes.

---

### Phase 1: Beads Bridge (Proposal A, minimal) [NEXT]

**Goal:** OJ jobs become visible as beads. One-way sync: OJ → beads.

**Deliverables:**
- On `JobCreated`: OJ runs `bd create` with `oj:job` label
- On step transitions: OJ runs `bd update` with step label
- On `JobCompleted`/`JobFailed`: OJ runs `bd close` or adds `oj:failed` label
- New OJ config option: `beads_sync = true|false`

**Validation workflow:**
```
1. oj run gt-sling --var issue=test-123 ...
2. bd list -l oj:job           → see the job as a bead
3. Watch step labels change     → oj:step:provision → oj:step:execute → ...
4. Job completes                → bead auto-closes
5. bd show <job-bead>          → full lifecycle visible
```

**Assumption tested:** `bd` CLI is fast enough for synchronous calls in OJ's event
path. If too slow, we know to batch or async the writes.

**What we learn:** How much latency `bd` adds. Whether bead state is useful to
witness/deacon. Whether the label taxonomy works.

---

### Phase 2: Beads-Backed Queues [AFTER PHASE 1]

**Goal:** External queues that already poll beads become push-based via event bridge.

**Deliverables:**
- Event bridge watches `bd activity --follow`
- When merge-request bead created → push to OJ `merge-requests` queue
- When bug bead created with `ready` status → push to OJ `bugs` queue
- OJ workers still poll as fallback (belt + suspenders)

**Validation workflow:**
```
1. bd create -t merge-request "Test MR" --status open
2. Within 1 second: oj job list shows new merge job (event-driven)
   vs. within 30 seconds: (old polling behavior)
3. Kill event bridge → workers fall back to polling (no data loss)
4. Restart bridge → push resumes from WAL sequence
```

**Assumption tested:** Event-driven dispatch is reliable and faster than polling.
If `bd activity` is unreliable or lossy, we know to keep polling as primary.

**What we learn:** Real-world latency improvement. Whether the fallback model works.
Whether `bd activity --follow` handles reconnection gracefully.

---

### Phase 3: Bidirectional Events + GT Feed [AFTER PHASE 2]

**Goal:** OJ events appear in GT's feed. GT has real-time visibility into OJ work.

**Deliverables:**
- Event bridge transforms OJ events → GT `.events.jsonl` format
- GT feed shows: "polecat Toast started job bug-fix-123", "step advanced to test",
  "agent idle for 3m", "job completed"
- Witness patrol can react to OJ events instead of polling witness-inbox

**Validation workflow:**
```
1. gt sling --engine=oj test-issue gastown
2. gt feed                    → see OJ job milestones in real-time
3. Simulate agent idle        → witness sees event, nudges agent
4. Compare: old witness-inbox poll latency vs event-driven latency
```

**Assumption tested:** Unified event stream is useful for operational visibility.
If the feed is too noisy, we know to add filtering. If witness doesn't benefit
from faster events, we know hooks (Phase 4) are the real need.

**What we learn:** Whether real-time OJ visibility changes operator behavior.
What event granularity is useful vs noise.

---

### Phase 4: Lifecycle Hooks [AFTER PHASE 3]

**Goal:** GT can influence OJ execution decisions at key points.

**Deliverables:**
- OJ exposes hook registration API (pending research on what already exists)
- GT registers `on_job_failed` hook → decides retry/escalate based on bead state
- GT registers `filter_items` hook → reorders queue items by dependency graph
- Hooks are WAL-durable with timeout + fallback-to-default

**Validation workflow:**
```
1. Register on_job_failed hook → GT auto-retries on first failure
2. Intentionally fail a job    → verify GT decides "retry"
3. Fail again                  → verify GT decides "escalate"
4. Kill GT hook server         → verify OJ falls back to default behavior
5. Register filter_items hook  → dispatch P0 bug before P2 chore
6. Push 3 items simultaneously → verify ordering matches GT priority
```

**Assumption tested:** External hooks don't degrade OJ performance. GT's graph
knowledge produces better decisions than OJ's defaults.

**What we learn:** Hook latency overhead. Whether GT actually makes better retry
decisions. Whether priority ordering matters in practice.

---

### Phase 5: Full State Unification [LONG TERM]

**Goal:** Beads are the *authoritative* state for OJ jobs, not just a mirror.

**Deliverables:**
- OJ can reconstruct job state from beads on crash recovery
- OJ WAL becomes optimization layer, beads is source of truth
- Queue state lives in beads (persisted queues backed by beads)
- Single query layer for all work state: `bd list` covers everything

**Validation workflow:**
```
1. Run 5 concurrent OJ jobs
2. Kill OJ daemon hard (SIGKILL)
3. Restart daemon → reconstructs state from beads
4. All 5 jobs resume correctly
5. Compare: WAL recovery time vs beads recovery time
```

**Assumption tested:** Beads/Dolt is performant enough to be OJ's primary state
store. If recovery is too slow, beads stays as mirror (Phase 1) and WAL stays
authoritative.

**What we learn:** Whether Dolt's performance profile works for OJ's write
patterns. Whether the mapping is complete enough for full state reconstruction.

---

### Migration Path Summary

```
Phase 0: Safety     → OJ+GT coexist without data loss
Phase 1: Mirror     → OJ jobs visible as beads (one-way sync)
Phase 2: Push       → Event-driven queue dispatch (replace polling)
Phase 3: Observe    → Unified event feed (GT sees OJ in real-time)
Phase 4: Control    → GT hooks into OJ decisions (influence execution)
Phase 5: Unify      → Beads is authoritative state (full fusion)
```

Each phase is independently valuable and shippable. If we stop at Phase 1, we
still get operational visibility. If we stop at Phase 3, we have a responsive
event-driven system. Phase 5 is the end goal but not required for the system to
work.

---

## Relationship to Existing Work

This research builds on:

- **od-vq6** (Integration Hardening) -- 6 tactical fixes for immediate safety
- **od-ki9** (Convergence Deduplication) -- 10 tasks to eliminate duplication

The proposals here are *strategic* -- they define the target architecture that the
tactical work is moving toward:

```
od-vq6 (safety fixes)     → Prevent immediate harm
od-ki9 (deduplication)     → Remove redundancy
od-ki9 deep integration    → Define target architecture  ← THIS DOC
```

---

## Appendix: Research Agent Findings

### A1: OJ Hook/Extension Point Analysis

**What OJ has:**
- Internal lifecycle hooks: `on_done`, `on_fail`, `on_cancel` (step transitions)
- Agent handlers: `on_idle`, `on_dead`, `on_error`, `on_prompt` (state machine actions)
- `notify` block: `on_start`, `on_done`, `on_fail` (desktop notifications only)
- `AgentAction` enum: Nudge, Done, Fail, Resume, Escalate, Gate
- Shell effects for arbitrary commands (can call GT endpoints from job steps)
- Mutation API: `job_resume`, `job_cancel`, `queue_push`, `worker_resize`

**What OJ lacks:**
- No webhook/HTTP callback mechanism
- No middleware/interceptor pipeline
- No generic "call external system" effect
- No plugin API or loader
- No event subscriptions for external consumers
- No concurrency adjustment hooks
- No queue item filtering hooks

**Best integration approaches (no OJ code changes):**
1. Custom job steps with `on_fail` routing to GT-calling shell commands
2. WAL event replay (external service reads WAL, pushes to GT)
3. Queue-based integration (GT controls queue list/take commands)

**Best integration approaches (with OJ changes):**
1. Custom `NotifyAdapter` implementation that POSTs to GT webhooks
2. New `Effect::InvokeHook` with WAL durability + timeout + fallback
3. Event subscription API on the daemon socket

**Impact on Proposal C:** OJ needs new code for proper lifecycle hooks.
The existing `notify` mechanism could be extended (custom NotifyAdapter)
as a low-friction first step. Full hook API (filter_items, on_job_failed
with decision response) requires new Effect types and handler changes.

### A2: OJ Queue Unification Analysis

**Key finding: Beads is ALREADY the queue backend.**

All Gas Town queues in `infra.hcl` are external queues backed by `bd list`:
- `merge-requests`: `bd list -t merge-request --status open --json`
- `bugs`: `bd list -t bug --status open --no-assignee --json`
- `ready-work`: `bd ready --json`
- `witness-inbox`: `bd list -t message --label to:witness --json`
- `deacon-inbox`: `bd list -t message --label to:deacon --json`

Take operations use `bd update --status in_progress --assignee <worker>`.

**Two queue types in OJ:**
1. **External** (shell polling): Already beads-backed. Stateless in OJ.
2. **Persisted** (WAL-backed): In-memory state, ~1ms polling, ~1000 items/sec.
   Not used in Gas Town production.

**External queue take atomicity:** Non-atomic by design. Safety depends on
`bd update` being atomic (it is — single SQL write). OJ guards with
`inflight_items` HashSet to prevent duplicate dispatch.

**Performance:** `bd list` ~50ms, `bd update` ~10ms. Gas Town load is ~5-10
items/min. Beads is comfortably fast enough. Persisted queues would only help
at >100 items/sec (unlikely in GT context).

**Unification path:**
1. Already done: external queues poll beads
2. Next: event bridge pushes on bead creation (replace polling with push)
3. Later: retire OJ persisted queues in GT context (beads IS the state machine)
4. Keep: WAL-based persisted queues as fallback for OJ-standalone use

**Queue state machine migration:**
- `QueueItem.status` maps directly to bead status (open/in_progress/closed)
- `QueueItem.failure_count` maps to bead metadata field
- Retry logic moves from OJ timers to beads status transitions
- `bd update --expect-status open` provides CAS atomicity for claiming

---

## Appendix: Key Code References

| System | File | Lines | What |
|---|---|---|---|
| OJ | `crates/storage/src/state.rs` | 256 | MaterializedState::apply_event() |
| OJ | `crates/core/src/event.rs` | 1-1023 | Full event taxonomy (~50 types) |
| OJ | `crates/core/src/effect.rs` | 1-160 | Effect enum (side effects) |
| OJ | `crates/daemon/src/event_bus.rs` | 1-122 | WAL-backed event bus |
| OJ | `crates/engine/src/runtime/handlers/worker/` | * | Worker polling/dispatch |
| OJ | `crates/runbook/src/queue.rs` | 1-70 | Queue type definitions |
| GT | `gastown/internal/cmd/sling_oj.go` | 37-102 | GT→OJ dispatch bridge |
| GT | `gastown/internal/witness/handlers.go` | 761 | AutoNukeIfClean() |
| GT | `gastown/internal/tui/feed/events.go` | 1-607 | GT event feed |
| Shared | `library/gastown/infra.hcl` | 1-73 | External queue definitions |
| Shared | `library/gastown/sling.hcl` | 1-192 | GT-OJ sling runbook |
