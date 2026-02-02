# Pipeline `on_cancel` Transition

## Overview

Add an `on_cancel` field to pipeline step definitions so that when a pipeline is cancelled, it can route to a cleanup step instead of immediately transitioning to the terminal "cancelled" state. This mirrors the existing `on_fail` pattern: step-level `on_cancel` takes priority, pipeline-level `on_cancel` is the fallback, and terminal cancellation is the default when neither is configured. A step running as a cancel cleanup is non-cancellable — it runs to completion or failure.

## Project Structure

Key files to modify:

```
crates/
├── runbook/src/pipeline.rs      # Add on_cancel to StepDef and PipelineDef
├── runbook/src/pipeline_tests.rs # Parse tests for on_cancel in HCL and TOML
├── core/src/pipeline.rs          # Add cancelling flag to Pipeline
├── engine/src/steps.rs           # Add cancellation_transition_effects()
├── engine/src/steps_tests.rs     # Unit tests for new effects function
├── engine/src/runtime/pipeline.rs # Rework cancel_pipeline() to check on_cancel
```

## Dependencies

No new external dependencies. All changes use existing types (`StepTransition`, `Effect`, `Event`).

## Implementation Phases

### Phase 1: Data Model — `on_cancel` in Runbook Definitions

Add the `on_cancel` field to `StepDef` and `PipelineDef` in `crates/runbook/src/pipeline.rs`.

**`StepDef`** (line ~126, after `on_fail`):
```rust
/// Step to route to when the pipeline is cancelled during this step
#[serde(default)]
pub on_cancel: Option<StepTransition>,
```

**`PipelineDef`** (line ~174, after `on_fail`):
```rust
/// Step to route to when the pipeline is cancelled (no step-level on_cancel)
#[serde(default)]
pub on_cancel: Option<StepTransition>,
```

Both use `Option<StepTransition>` which already supports bare string `"name"` and structured `{ step = "name" }` forms via serde. HCL and TOML parsing requires no parser changes — serde handles the new optional field automatically.

**Tests** in `crates/runbook/src/pipeline_tests.rs`: Add TOML and HCL parse tests that include `on_cancel` at both step and pipeline levels, verifying it round-trips correctly. Follow the existing test patterns (lines ~84-146).

**Milestone**: `cargo test -p oj-runbook` passes with new on_cancel parse tests.

### Phase 2: Cancelling Flag in Core Pipeline

Add a flag to `Pipeline` in `crates/core/src/pipeline.rs` to track when a pipeline is running its cancel cleanup step. This prevents re-cancellation.

```rust
/// True when running an on_cancel cleanup step. Prevents re-cancellation.
#[serde(default)]
pub cancelling: bool,
```

Add to `Pipeline::new_with_epoch_ms` with default `false`.

**Milestone**: `cargo test -p oj-core` passes, `cargo clippy` clean.

### Phase 3: Cancellation Transition Effects

Add a new `cancellation_transition_effects()` function in `crates/engine/src/steps.rs`, mirroring `failure_transition_effects()`:

```rust
/// Build effects to transition to a cancel-cleanup step (non-terminal).
///
/// Records the cancellation of the current step, then advances to the
/// on_cancel target. The pipeline remains non-terminal so the cleanup
/// step can execute.
pub fn cancellation_transition_effects(
    pipeline: &Pipeline,
    on_cancel_step: &str,
) -> Vec<Effect> {
    let pipeline_id = PipelineId::new(&pipeline.id);
    vec![
        Effect::Emit {
            event: Event::StepFailed {
                pipeline_id: pipeline_id.clone(),
                step: pipeline.step.clone(),
                error: "cancelled".to_string(),
            },
        },
        Effect::Emit {
            event: Event::PipelineAdvanced {
                id: pipeline_id,
                step: on_cancel_step.to_string(),
            },
        },
    ]
}
```

Add unit tests in `crates/engine/src/steps_tests.rs` verifying the effects contain `StepFailed` with "cancelled" error and `PipelineAdvanced` to the target step (no timer cancellation, no session kill — those happen inline in `cancel_pipeline`).

**Milestone**: `cargo test -p oj-engine` passes for new effects tests.

### Phase 4: Runtime Cancel Logic

Rework `cancel_pipeline()` in `crates/engine/src/runtime/pipeline.rs` (lines 453-472) to mirror the `fail_pipeline()` pattern:

```rust
pub(crate) async fn cancel_pipeline(
    &self,
    pipeline: &Pipeline,
) -> Result<Vec<Event>, RuntimeError> {
    if pipeline.is_terminal() {
        return Ok(vec![]);
    }

    // If already running a cancel cleanup step, don't re-cancel — let it finish
    if pipeline.cancelling {
        tracing::info!(pipeline_id = %pipeline.id, "cancel: already running cleanup, ignoring");
        return Ok(vec![]);
    }

    let runbook = self.cached_runbook(&pipeline.runbook_hash)?;
    let pipeline_def = runbook.get_pipeline(&pipeline.kind);
    let current_step_def = pipeline_def
        .as_ref()
        .and_then(|p| p.get_step(&pipeline.step));
    let on_cancel = current_step_def.and_then(|s| s.on_cancel.as_ref());

    let pipeline_id = PipelineId::new(&pipeline.id);

    // Cancel timers and kill session (same cleanup as fail_pipeline for agent steps)
    let current_is_agent = current_step_def
        .map(|s| matches!(&s.run, RunDirective::Agent { .. }))
        .unwrap_or(false);
    if current_is_agent {
        self.executor.execute(Effect::CancelTimer {
            id: TimerId::liveness(&pipeline_id),
        }).await?;
        self.executor.execute(Effect::CancelTimer {
            id: TimerId::exit_deferred(&pipeline_id),
        }).await?;

        if let Some(agent_id) = pipeline
            .step_history.iter()
            .rfind(|r| r.name == pipeline.step)
            .and_then(|r| r.agent_id.as_ref())
        {
            self.agent_pipelines.lock().remove(&oj_core::AgentId::new(agent_id));
        }

        if let Some(session_id) = &pipeline.session_id {
            let sid = SessionId::new(session_id);
            self.executor.execute(Effect::KillSession { session_id: sid.clone() }).await?;
            self.executor.execute(Effect::Emit {
                event: Event::SessionDeleted { id: sid },
            }).await?;
        }
    }

    let mut result_events = Vec::new();

    if let Some(on_cancel) = on_cancel {
        // Step-level on_cancel: route to cleanup step
        let target = on_cancel.step_name();
        // Set cancelling flag via event (see Phase 5)
        result_events.extend(
            self.executor.execute(Effect::Emit {
                event: Event::PipelineCancelling { id: pipeline_id.clone() },
            }).await?,
        );
        let effects = steps::cancellation_transition_effects(pipeline, target);
        result_events.extend(self.executor.execute_all(effects).await?);
        result_events.extend(
            self.start_step(&pipeline_id, target, &pipeline.vars, &self.execution_dir(pipeline)).await?,
        );
    } else if let Some(ref pipeline_on_cancel) =
        pipeline_def.as_ref().and_then(|p| p.on_cancel.clone())
    {
        // Pipeline-level on_cancel fallback
        let target = pipeline_on_cancel.step_name();
        if pipeline.step != target {
            result_events.extend(
                self.executor.execute(Effect::Emit {
                    event: Event::PipelineCancelling { id: pipeline_id.clone() },
                }).await?,
            );
            let effects = steps::cancellation_transition_effects(pipeline, target);
            result_events.extend(self.executor.execute_all(effects).await?);
            result_events.extend(
                self.start_step(&pipeline_id, target, &pipeline.vars, &self.execution_dir(pipeline)).await?,
            );
        } else {
            // Already at the cancel target; go terminal
            let effects = steps::cancellation_effects(pipeline);
            result_events.extend(self.executor.execute_all(effects).await?);
        }
    } else {
        // No on_cancel configured; terminal cancellation as before
        let effects = steps::cancellation_effects(pipeline);
        result_events.extend(self.executor.execute_all(effects).await?);
    }

    tracing::info!(pipeline_id = %pipeline.id, "cancelled pipeline");
    Ok(result_events)
}
```

When the cancel-cleanup step completes (via `advance_pipeline`) or fails (via `fail_pipeline`), it proceeds to terminal "cancelled" or "failed" as normal. The `cancelling` flag only gates re-cancellation — it does not alter completion/failure routing.

**Milestone**: Cancel with `on_cancel` routes to cleanup step; cancel without `on_cancel` goes terminal as before.

### Phase 5: `PipelineCancelling` Event and State Update

Add a new event variant in `crates/core/src/event.rs`:

```rust
#[serde(rename = "pipeline:cancelling")]
PipelineCancelling { id: PipelineId },
```

Handle it in `crates/storage/src/state.rs` `apply_event()`:

```rust
Event::PipelineCancelling { id } => {
    if let Some(pipeline) = state.pipelines.get_mut(id.as_str()) {
        pipeline.cancelling = true;
    }
}
```

Wire the event through the handler dispatch in `crates/engine/src/runtime/handlers/mod.rs` (no-op for the handler — the event is purely for state mutation via WAL).

When the cleanup step finishes (done or fail), the pipeline transitions to terminal state. The `cancelling` flag persists but is irrelevant once terminal.

**Milestone**: `cancelling` flag survives WAL replay. Re-cancel during cleanup is a no-op.

### Phase 6: Tests and Documentation

**Unit tests** (`crates/engine/src/steps_tests.rs`):
- `cancellation_transition_effects` emits `StepFailed` + `PipelineAdvanced` to target
- `cancellation_transition_effects` does NOT cancel timers or kill sessions (that's the runtime's job)

**Integration-style tests** (can be unit tests using `FakeAdapters` in `crates/engine/`):
- Cancel pipeline with step-level `on_cancel` → transitions to cleanup step, `cancelling` is true
- Cancel pipeline with pipeline-level `on_cancel` → transitions to cleanup step
- Cancel pipeline with no `on_cancel` → terminal "cancelled" (existing behavior preserved)
- Cancel pipeline that is already `cancelling` → no-op
- Cancel-cleanup step completes → pipeline goes terminal (done or cancelled depending on `advance_pipeline` routing — when `cancelling` is true and no explicit `on_done`, the terminal state should be "cancelled")

**Docs**: Update `docs/` with a section on `on_cancel` usage in runbook definitions.

**Milestone**: `make check` passes (fmt, clippy, tests, build, audit, deny).

## Key Implementation Details

### Pattern: Mirror `on_fail`

The `on_cancel` implementation follows `on_fail` exactly:
1. Step-level field checked first
2. Pipeline-level field as fallback
3. Terminal state when neither is set
4. Cleanup (timers, session kill, agent deregistration) happens before routing

### Non-cancellable cleanup steps

The `cancelling: bool` flag on `Pipeline` prevents re-cancellation. When `cancel_pipeline()` sees `cancelling == true`, it returns `Ok(vec![])`. This means the cleanup step runs to completion or failure — it cannot be cancelled again.

### Terminal state after cleanup

When a cancel-cleanup step finishes:
- **Step succeeds** (`advance_pipeline`): If `cancelling` is true and the step has no `on_done`, the pipeline should advance to "cancelled" instead of "done". This requires a small addition to `advance_pipeline`: check `pipeline.cancelling` when determining the terminal step name.
- **Step fails** (`fail_pipeline`): Normal failure routing applies. If the cleanup step has `on_fail`, it routes there. Otherwise terminal "failed".

### WAL compatibility

The new `cancelling` field uses `#[serde(default)]` so existing WAL entries deserialize with `cancelling: false`. The new `PipelineCancelling` event is additive. No migration needed.

### No changes to `is_terminal()`

The "cancelled" terminal state already exists. No new terminal states are introduced.

## Verification Plan

1. **Phase 1**: `cargo test -p oj-runbook` — on_cancel parses from HCL and TOML
2. **Phase 2**: `cargo test -p oj-core` — Pipeline struct compiles with new field
3. **Phase 3**: `cargo test -p oj-engine` — cancellation_transition_effects tests pass
4. **Phase 4-5**: `cargo test -p oj-engine` — cancel_pipeline routing tests pass
5. **Phase 6**: `make check` — full suite (fmt, clippy, quench, test, build, audit, deny)
6. **Manual**: Create a test runbook with `on_cancel` step, run a pipeline, cancel it, verify cleanup step executes
