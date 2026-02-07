# od-ki9 Dispatch Plan: GT/OJ Convergence

## Status

- **Epic**: od-ki9 (GT/OJ Convergence: Deduplicate and establish ownership)
- **Phase**: ki9.3 DONE. All polecats dead (API limits + BD_DAEMON_HOST). Needs re-dispatch.
- **Prior research**: GT_OJ_CONVERGENCE_ANALYSIS.md, od-ki9-deep-integration-research.md
- **Prior spikes**: od-vq6.1 (witness handoff), od-vq6.2 (merge queue ownership)
- **Last updated**: 2026-02-07T05:00 by oddjobs/crew/research

## Polecat Status (all dead — need re-dispatch)

| Bead | Last Polecat | Rig | Wave | Status |
|------|-------------|-----|------|--------|
| ki9.1 | gastown/nux | gastown | 1 | Dead (API limit). Nux was on P1 task first. |
| ki9.3 | — | — | 1 | **DONE** — crew/research implemented (9a96059) |
| ki9.9 | oddjobs/quartz | oddjobs | 1 | Dead. Had +301 lines health endpoint, lost before push. |
| ki9.4 | gastown/dag | gastown | 2 | Dead (API limit). Was reading source files. |
| ki9.5 | gastown/dementus | gastown | 2 | Dead (API limit). Was running tmux tests. |
| ki9.6 | gastown/keeper | gastown | 2 | Dead. Was running go test. |

### Known Issues
- **gt-8vf4tu**: gt sling hook step fails for oddjobs polecats when BD_DAEMON_HOST is set
- Agent bead creation warns "missing gt:agent label" on gastown polecats — non-fatal
- BD daemon intermittently returns 503 under load — retry after 10s
- **API limits**: Gastown polecats hit Anthropic API limits (resets 9am UTC)
- **Oddjobs polecats**: Repeated lifecycle failures from BD_DAEMON_HOST bug

## Dependency Graph

```
Phase 1: Stop the Bleeding
  ki9.1  Stop GT tmux spawn for OJ polecats     [READY - no deps]
  ki9.3  Flag OJ merge.hcl as legacy             [READY - no deps]
  ki9.9  OJ daemon health endpoint               [READY - no deps, moved up]
  ki9.2  Witness queries OJ for health            [BLOCKED by ki9.9]

Phase 2: Clean Integration Seams
  ki9.4  Canonical nudge via OJ agent send        [READY - no deps]
  ki9.5  Extract PGID kill to shared utility      [READY - no deps]
  ki9.6  Unify environment injection              [READY - no deps]
  ki9.7  Consolidate bypass-permissions           [BLOCKED by ki9.4]

Phase 3: Feature Migration
  ki9.8  Formulas generate OJ runbook invocations [BLOCKED by ki9.6]
  ki9.10 Consolidate queue polling                [BLOCKED by ki9.9]
```

## Dispatch Ordering

### Wave 1 (DISPATCHED 2026-02-07 — Phase 1 guards + unblocking ki9.9)

| Bead | Target Rig | Polecat | Status |
|------|-----------|---------|--------|
| **ki9.1** | gastown | nux | Dispatched, queued behind P1 |
| **ki9.3** | oddjobs | obsidian | Dispatched, actively researching |
| **ki9.9** | oddjobs | quartz | Dispatched, actively researching |

### Wave 2 (DISPATCHED 2026-02-07 — Phase 2 seams, parallel with Wave 1)

| Bead | Target Rig | Polecat | Status |
|------|-----------|---------|--------|
| **ki9.4** | gastown | dag | Dispatched with detailed research findings |
| **ki9.5** | gastown | cheedo | Dispatched with detailed research findings |
| **ki9.6** | gastown | keeper | Dispatched with detailed research findings |
| **ki9.2** | gastown | — | BLOCKED by ki9.9 (awaiting health endpoint) |

### Wave 3 (After Wave 2 — Phase 2 blocked + Phase 3)

| Bead | Target Rig | Primary Language | Rationale |
|------|-----------|-----------------|-----------|
| **ki9.7** | oddjobs | Rust | OJ adapter consolidation (needs ki9.4 canonical path). |
| **ki9.8** | gastown | Go + HCL | Capstone: GT formulas emit OJ runbook invocations (needs ki9.6). |
| **ki9.10** | oddjobs | Rust/HCL | OJ queue polling canonical (needs ki9.9). |

## Cross-References

### od-vq6 Spike Reports → ki9 Children

| Spike | Feeds Into | Key Findings |
|-------|-----------|--------------|
| od-vq6.1 (Witness handoff) | **ki9.1**, **ki9.2** | `oj_job_id` discriminator pattern. 5 edge cases. State machine diagrams. |
| od-vq6.2 (Merge queue ownership) | **ki9.3** | Single-writer model. 6-phase mitigation. Locking protocol. |
| od-vq6.3 (Version headers) | General | Mismatch detection for runbooks. |
| od-vq6.4 (Auto-close beads) | **ki9.3** related | Merge job → bead lifecycle wiring. |
| od-vq6.5 (Integration tests) | All | Cross-system test suite for validation. |
| od-vq6.6 (Gitignore) | **ki9.3** related | Generated gt-sling.hcl untracked. |

## Acceptance Criteria Per Child

### ki9.1 — Stop GT tmux spawn for OJ polecats
- [ ] `AutoNukeIfClean()` in `handlers.go` checks `oj_job_id` on bead
- [ ] If `oj_job_id` present, skip tmux kill-session and worktree delete
- [ ] Log message indicates OJ-managed polecat detected
- [ ] Existing GT-native polecats unaffected
- **Ref**: od-vq6.1 spike report, handlers.go:761

### ki9.3 — Flag OJ merge.hcl as legacy
- [ ] merge.hcl checks `OJ_LEGACY_MERGE` env var at job start
- [ ] If not set (or `!=1`), job exits with warning message
- [ ] GT refinery continues as canonical merge owner
- [ ] Document the guard in merge.hcl comments
- **Ref**: od-vq6.2 spike report, merge.hcl

### ki9.9 — OJ daemon health endpoint
- [ ] OJ daemon exposes `/health` or equivalent IPC endpoint
- [ ] Returns: worker count, queue depth, running jobs, daemon uptime
- [ ] Per-job status queryable: given job_id, return state + agent state
- [ ] GT doctor can query this endpoint
- **Ref**: Convergence analysis section E (Monitoring/Health)

### ki9.2 — Witness queries OJ daemon for health
- [ ] Witness checks bead for `oj_job_id` before health check
- [ ] If present, queries ki9.9 health endpoint instead of tmux
- [ ] Maps OJ agent states to GT health model
- [ ] Falls back to tmux check if OJ daemon unreachable
- **Ref**: od-vq6.1 spike, convergence analysis section E

### ki9.4 — Canonical nudge via OJ agent send
- [ ] `gt nudge` routes to `oj agent send` when `oj_job_id` present
- [ ] GT nudge remains for GT-native polecats (no oj_job_id)
- [ ] Both paths produce equivalent agent behavior
- **Ref**: Convergence analysis section A (tmux.go:862 vs claude.rs:160)

### ki9.5 — Extract PGID kill to shared utility
- [ ] GT's PGID-aware kill logic extracted from tmux.go:255-312
- [ ] Available as standalone utility or shared package
- [ ] OJ can optionally consume it (future work)
- [ ] GT tmux.go imports from extracted location
- **Ref**: Convergence analysis section H (Shutdown/Lifecycle)

### ki9.6 — Unify environment injection
- [ ] GT's `AgentEnv()` produces canonical env map
- [ ] OJ's spawn reads GT-produced env map as input
- [ ] No duplicate env construction in OJ's claude.rs:380-430
- [ ] Both GT and OJ agents get identical environment
- **Ref**: Convergence analysis section F (Configuration)

### ki9.7 — Consolidate bypass-permissions
- [ ] OJ's pattern (send "2") is canonical for OJ-managed agents
- [ ] GT's pattern (Down+Enter) preserved for GT-native agents
- [ ] Timing/retry edge cases from GT preserved in OJ path
- **Ref**: Convergence analysis section A (tmux.go:967 vs claude.rs:128)

### ki9.8 — Formulas generate OJ runbook invocations
- [ ] GT formulas emit `oj run <command>` instead of direct execution
- [ ] GT remains orchestration layer (what to run)
- [ ] OJ runbooks handle execution (how to run)
- [ ] Existing formula behavior preserved for GT-native paths
- **Ref**: Convergence analysis section J (Runbook vs Formula)

### ki9.10 — Consolidate queue polling
- [ ] OJ infra.hcl is canonical queue poller
- [ ] GT Poller dispatches to OJ rather than polling directly
- [ ] No duplicate polling of same beads sources
- [ ] Race conditions from dual polling eliminated
- **Ref**: Convergence analysis section I (Queue Infrastructure)

## Risk Assessment

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| ki9.1 guard misses edge case | Medium | HIGH (false nuke) | Test with both OJ and GT polecats running |
| ki9.3 merge.hcl guard breaks gastown dev | Low | Medium | Only disables outside gastown namespace |
| ki9.9 health endpoint incomplete | Medium | Medium | ki9.2 has fallback to tmux check |
| Cross-rig changes conflict | Medium | Medium | Dispatch waves prevent concurrent edits |
| OJ daemon not running when queried | Medium | HIGH | All GT→OJ queries need timeout + fallback |

## Notes

- **ki9.9 promoted to Wave 1**: Originally Phase 3, but it unblocks both ki9.2 (Phase 1) and ki9.10 (Phase 3). Promoting it accelerates the entire graph.
- **Cross-rig children (ki9.4, ki9.6)**: May need splitting into sub-tasks per rig, or a single polecat with worktree access to both repos.
- **od-vq6 overlap**: vq6.1 and vq6.2 are research-only spikes. Their findings feed ki9 children. The vq6 beads can be closed as "superseded by ki9" once ki9 children land.
