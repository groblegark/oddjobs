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

**Critical discovery: Beads already has an event bus.** The `bd bus` system provides:
- Priority-based handler dispatch with blocking/injection
- Embedded NATS JetStream (`BD_NATS_ENABLED=true`) for distributed pub/sub
- `bd bus emit --hook=<type>` for event dispatch (RPC-first, local fallback)
- Active rollout project at `beads/crew/bus_rollout/` with full docs

This means we don't need a new event-bridge crate. OJ can emit lifecycle events
via `bd bus emit`, and GT handlers subscribe through the existing NATS infrastructure.
The event bus already supports hook types like `SessionStart`, `PreToolUse`, etc. --
we'd extend this with OJ-specific types: `OjJobCreated`, `OjStepAdvanced`,
`OjAgentIdle`, `OjJobCompleted`, `OjJobFailed`.

**Revised effort:** Low-Medium (1-2 weeks). Define new event types + add `bd bus emit`
calls to OJ's event handlers. No new infrastructure needed.

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

**Beads event bus changes the picture:** With `bd bus` available, the simplest
hook approach is: OJ emits lifecycle events via `bd bus emit`, GT registers
handlers that respond. The bus already supports handler blocking (exit code 2
= block the event) and content injection. This could serve as a lightweight
hook mechanism without building a new OJ hook API:

- OJ emits `OjJobFailed` → GT handler decides retry/escalate → returns action
- OJ emits `OjWorkerPollComplete` → GT handler reorders items → returns sorted list
- Bus handlers run with priority ordering, so GT can intercept before OJ defaults

**Key consideration:** Bus handlers must be non-blocking. The beads bus already
handles errors gracefully (logs error, continues chain).

**Effort:** Low-Medium. Define event types + handlers. No new OJ hook API needed
if the bus is sufficient.

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
| 2 | **B: bd bus as OJ-GT Event Plane** | High | Low-Medium | Ready (infra exists) |
| 3 | **C: GT Lifecycle Hooks via Bus** | Medium-High | Low-Medium | Ready (bus = hooks) |
| 4 | D: Dynamic Runbooks | Medium | High | Deferred |
| 5 | E: Shared Agent Context | Low | Medium | Not needed |

**Key insight: Proposals A+B+C collapse into one integration surface.** OJ emits
lifecycle events via `bd bus emit`, which simultaneously:
- Updates bead state (Proposal A) -- via bus handler that calls `bd update`
- Notifies GT in real-time (Proposal B) -- via NATS subscription
- Enables GT to influence decisions (Proposal C) -- via handler blocking/injection

This means we can implement all three as one coordinated effort using `bd bus`.

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

### Phase 1: OJ Emits to bd bus [NEXT]

**Goal:** OJ lifecycle events flow through the beads event bus. This is the
foundation — once OJ speaks to `bd bus`, everything else layers on top.

**What we build in OJ:**

Add a `BeadsBusEmitter` to OJ's runtime that calls `bd bus emit` on key events.
This lives in the OJ executor alongside the existing Shell/Notify/Agent effects:

```
OJ Event fires
  → Runtime handler produces effects (existing)
  → NEW Effect::BusEmit { hook_type, payload }
  → Executor runs: echo '<payload>' | bd bus emit --hook=<type>
  → Bus dispatches to registered handlers (NATS + local)
```

**New OJ event types for the bus:**

| Bus Hook Type | Fires When | Payload |
|---|---|---|
| `OjJobCreated` | Job starts | job_id, name, namespace, vars, runbook |
| `OjStepAdvanced` | Step transitions | job_id, from_step, to_step |
| `OjAgentSpawned` | Agent starts | job_id, agent_id, model, session_id |
| `OjAgentIdle` | Agent idle > grace | job_id, agent_id, idle_duration |
| `OjAgentEscalated` | Decision created | job_id, agent_id, decision_id |
| `OjJobCompleted` | Job succeeds | job_id, duration, step_history |
| `OjJobFailed` | Job fails | job_id, step, exit_code, stderr |
| `OjWorkerPollComplete` | Queue polled | worker, queue, item_count, items[] |

**OJ-side implementation (Rust):**

```rust
// New file: crates/adapters/src/bus.rs
pub async fn emit_bus_event(hook_type: &str, payload: &serde_json::Value) -> Result<()> {
    let json = serde_json::to_string(payload)?;
    let mut child = Command::new("bd")
        .args(["bus", "emit", "--hook", hook_type])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    child.stdin.take().unwrap().write_all(json.as_bytes()).await?;
    let output = child.wait_with_output().await?;
    // exit code 2 = handler blocked the event
    Ok(())
}
```

**Where to hook in OJ's runtime:**
- `crates/engine/src/runtime/handlers/job_create.rs` → emit `OjJobCreated`
- `crates/engine/src/runtime/handlers/lifecycle.rs` → emit `OjJobCompleted`/`OjJobFailed`
- `crates/engine/src/runtime/handlers/agent.rs` → emit `OjAgentSpawned`/`OjAgentIdle`
- `crates/engine/src/runtime/handlers/worker/dispatch.rs` → emit `OjWorkerPollComplete`

**Beads-side: register default handlers:**

```bash
# Handler: sync OJ job state to beads
bd bus register --hook OjJobCreated --priority 100 \
  --run 'bd create "${.name}" -l oj:job -l "oj:ns:${.namespace}" -d "${.vars}" --json'

bd bus register --hook OjStepAdvanced --priority 100 \
  --run 'bd label "${.job_bead_id}" +oj:step:${.to_step} -oj:step:${.from_step}'

bd bus register --hook OjJobCompleted --priority 100 \
  --run 'bd close "${.job_bead_id}" --reason "OJ job completed"'

bd bus register --hook OjJobFailed --priority 100 \
  --run 'bd update "${.job_bead_id}" -l oj:failed --status blocked'
```

**Validation workflow:**
```
1. oj run gt-sling --var issue=test-123 ...
2. bd list -l oj:job           → see the job as a bead (created by handler)
3. Watch step labels change     → oj:step:provision → oj:step:execute
4. Job completes                → bead auto-closes (handler fires)
5. bd bus status                → see event flow metrics
```

**Assumption tested:** `bd bus emit` latency is acceptable in OJ's hot path.
If too slow, we async the emit (fire-and-forget) or batch.

**What we learn:** Bus round-trip latency. Handler reliability. Whether the
event taxonomy is complete enough for GT's needs.

**Effort:** Low-Medium. ~1 week OJ side (new effect + 6-8 emit sites), ~1 week
beads side (register handlers, test NATS delivery).

---

### Phase 2: GT Subscribes and Reacts [AFTER PHASE 1]

**Goal:** GT consumes OJ bus events for real-time visibility and queue push.

**Deliverables:**

1. **GT feed integration:** Bus handler transforms OJ events → GT `.events.jsonl`
   ```bash
   # Handler: OJ events appear in GT feed
   bd bus register --hook OjJobCreated --priority 50 \
     --run 'gt event emit --type polecat_slung --actor "oj/${.namespace}" ...'
   ```

2. **Push-based queue dispatch:** Bead creation triggers OJ queue push via bus
   ```bash
   # Handler: new merge-request bead → push to OJ merge queue
   bd bus register --hook BeadCreated --priority 100 \
     --filter 'type=merge-request' \
     --run 'oj queue push merge-requests --data "${.id}"'
   ```

3. **Witness reacts to OJ events:** Instead of polling witness-inbox
   ```bash
   # Handler: agent idle → witness nudges
   bd bus register --hook OjAgentIdle --priority 50 \
     --run 'gt witness nudge "${.agent_id}" --reason "idle ${.idle_duration}"'
   ```

4. OJ workers keep polling as fallback (belt + suspenders)

**Validation workflow:**
```
1. bd create -t merge-request "Test MR" --status open
2. Bus fires BeadCreated → handler pushes to OJ queue
3. Within 1s: oj job list shows merge job (bus-driven)
4. gt feed → see "merge job started" event
5. Disable handler → workers fall back to polling (no data loss)
```

**Assumption tested:** Bus-driven dispatch is reliable and faster than polling.

**Effort:** Low. Mostly handler registration and testing. GT feed integration
may need a small `gt event emit` CLI addition.

---

### Phase 3: GT Influences OJ Decisions via Bus [AFTER PHASE 2]

**Goal:** GT uses bus handler blocking/injection to influence OJ behavior.

The beads bus already supports **handler blocking** (exit code 2) and **content
injection** (stdout returned to emitter). This is our hook mechanism.

**Deliverables:**

1. **Smart retry on failure:**
   ```bash
   # Handler: GT decides retry vs escalate
   bd bus register --hook OjJobFailed --priority 200 \
     --run 'gt decide-retry "${.job_bead_id}" --exit-code "${.exit_code}"'
   # gt decide-retry returns: {"action":"retry"} or {"action":"escalate"}
   # OJ reads response, acts accordingly
   ```

2. **Priority reordering:**
   ```bash
   # Handler: GT reorders queue items by dependency graph
   bd bus register --hook OjWorkerPollComplete --priority 200 \
     --run 'gt reorder-queue "${.items}" --by priority,dependency'
   # Returns reordered items list; OJ dispatches in new order
   ```

3. **Dynamic concurrency:**
   ```bash
   # Handler: GT adjusts worker concurrency based on rig load
   bd bus register --hook OjWorkerStarted --priority 200 \
     --run 'gt recommend-concurrency "${.worker}" "${.namespace}"'
   # Returns {"concurrency": 3}; OJ applies
   ```

**OJ-side requirement:** OJ's `BusEmit` effect must read handler response
(stdout) and apply it. This requires extending the executor:

```rust
// In bus.rs:
pub struct BusResponse {
    pub blocked: bool,       // exit code 2
    pub payload: Option<serde_json::Value>,  // stdout JSON
}

// OJ runtime checks response:
if let Some(response) = bus_response.payload {
    if response["action"] == "retry" {
        return self.handle_job_resume(job_id, ...).await;
    }
}
```

**Validation workflow:**
```
1. Register OjJobFailed handler → GT auto-retries on first failure
2. Fail a job intentionally → GT decides "retry" → job resumes
3. Fail again → GT decides "escalate" → decision bead created
4. Kill GT handler → OJ falls back to default (fail the job)
```

**Assumption tested:** Handler round-trip is fast enough for synchronous decisions.
If too slow, split into fire-and-forget events (Phase 2) vs blocking hooks (Phase 3).

**Effort:** Medium. Requires OJ executor to read bus response + GT decision logic.

---

### Phase 4: Full State Unification [LONG TERM]

**Goal:** Beads are the *authoritative* state for OJ jobs, not just a mirror.

**Deliverables:**
- OJ can reconstruct job state from beads on crash recovery
- OJ WAL becomes optimization layer, beads is source of truth
- Queue state lives in beads (persisted queues retired in GT context)
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
store. If too slow, beads stays as mirror and WAL stays authoritative.

**Effort:** High. Full state mapping + recovery logic + testing.

---

### Migration Path Summary

```
Phase 0: Coexist    → OJ+GT clean boundaries (od-vq6)
Phase 1: Emit       → OJ speaks to bd bus (foundation)
Phase 2: React      → GT subscribes, push replaces polling
Phase 3: Influence  → GT hooks into OJ decisions via bus blocking
Phase 4: Unify      → Beads is authoritative state (full fusion)
```

Phases 1-3 are all bus-based and build incrementally on the same infrastructure.
Phase 1 is the critical path — once OJ emits to `bd bus`, phases 2 and 3 are
just handler registration.

**Dependencies:**
- Phase 1 requires: `bd bus` matured enough for OJ event types + NATS enabled
- Phase 2 requires: Phase 1 + GT feed integration
- Phase 3 requires: Phase 1 + OJ reads bus response + GT decision logic
- Phase 4 requires: Phase 1 proven stable at scale

**Work needed on bd bus itself:**
- Add OJ-specific hook types to the type registry
- Ensure `bd bus emit` performance is <50ms (currently unknown)
- Test NATS JetStream durability under OJ event volume (~50-100 events/job)
- Handler registration persistence (survive daemon restart)
- `bd bus register` CLI for declarative handler setup (may need building)

---

## Relationship to Existing Work

This research builds on:

- **od-vq6** (Integration Hardening) -- 6 tactical fixes for immediate safety
- **od-ki9** (Convergence Deduplication) -- 10 tasks to eliminate duplication
- **od-k3o** (BD Bus Maturation) -- 9 tasks to mature bd bus for OJ integration

The proposals here are *strategic* -- they define the target architecture that the
tactical work is moving toward:

```
od-vq6 (safety fixes)     → Prevent immediate harm
od-ki9 (deduplication)     → Remove redundancy
od-k3o (bus maturation)    → Build the integration surface
od-ki9 deep integration    → Define target architecture  ← THIS DOC
```

### od-k3o: BD Bus Maturation for OJ Integration

**Epic:** od-k3o | **Priority:** P1 | **Children:** 9 tasks

The bus assessment (2026-02-06) found a two-tier implementation: jasper and
refinery/rig have full bus support (RPC handlers, concrete handlers, daemon
registration, 19 tests). All other beads have protocol stubs only.

| # | Task | Priority | What |
|---|------|----------|------|
| od-k3o.1 | Enable NATS JetStream by default | P1 | Remove BD_NATS_ENABLED gate, connect NATS to dispatch |
| od-k3o.2 | Handler persistence | P2 | Survive daemon restarts |
| od-k3o.3 | Cross-bead rollout | P2 | server_bus.go + handlers in all beads |
| od-k3o.4 | Custom handler registration API | P1 | `bd bus register` for external process handlers |
| od-k3o.5 | Unit + integration tests | P1 | NATS, persistence, blocking, concurrency, benchmarks |
| od-k3o.6 | OJ-specific event types | P2 | OjJobCreated, OjStepAdvanced, etc. in type registry |
| od-k3o.7 | OJ bus emit implementation | P2 | Effect::BusEmit or subprocess calls in OJ runtime |
| od-k3o.8 | Default OJ event handlers | P2 | Bead sync on job complete/fail/escalate |
| od-k3o.9 | Latency benchmarks | P3 | <50ms target for emit-to-handler |

**Execution order:** k3o.1 + k3o.4 + k3o.5 first (bus infrastructure), then
k3o.6 + k3o.7 + k3o.8 (OJ integration), then k3o.2 + k3o.3 + k3o.9 (hardening).

**Predecessor:** bd-66fp "Event Bus Rollout" epic (CLOSED, assigned to
beads/polecats/onyx). Work described in bd-66fp phases 1-3 appears complete
in jasper/refinery but not rolled out. Open convoy: hq-cv-5vrii.

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

### A3: Beads Event Bus (Existing Infrastructure)

The beads system already has a mature event bus (`bd bus`) with:

**Core:** `beads/*/internal/eventbus/bus.go` — priority-based handler dispatch
**Types:** `beads/*/internal/eventbus/types.go` — hook event enum
**CLI:** `bd bus emit`, `bd bus status`, `bd bus handlers`
**NATS:** Embedded JetStream server (`BD_NATS_ENABLED=true`), stream `HOOK_EVENTS`,
subjects `hooks.<type>`, file storage with 10k/100MB limits
**RPC:** `OpBusEmit`, `OpBusStatus`, `OpBusHandlers` in daemon protocol
**Activity:** `bd activity --follow` uses fsnotify + polling fallback, 50ms debounce
**Rollout:** Active project at `beads/crew/bus_rollout/` with full docs and design research

**Existing hook types:** SessionStart, UserPromptSubmit, PreToolUse, PostToolUse,
PostToolUseFailure, Stop, PreCompact, SubagentStart, SubagentStop, Notification,
SessionEnd

**Proposed OJ hook types:** OjJobCreated, OjStepAdvanced, OjAgentIdle,
OjAgentEscalated, OjJobCompleted, OjJobFailed, OjWorkerPollComplete

**Key capability:** Bus handlers can BLOCK events (exit code 2) and INJECT content.
This enables GT to intercept OJ events and return decisions (retry, escalate, reorder).

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
