# Checkpoint Lock Contention — Future

## Problem

The checkpoint task (`daemon/src/main.rs`) acquires both `state.lock()`
and `event_wal.lock()` simultaneously to clone the materialized state and
read the WAL's processed sequence number:

```rust
let (state_ref, processed_seq) = {
    let state_guard = state.lock();
    let wal_guard = event_wal.lock();
    (state_guard.clone(), wal_guard.processed_seq())
};
```

While both locks are held, the event loop cannot `apply_event()` (needs
state lock) and the flush task cannot `flush()` (needs WAL lock). The
duration depends on the cost of `state_guard.clone()`, which grows with
the number of jobs, step histories, and workspace records.

After the clone, both locks are released. Snapshot I/O (serialization,
zstd compression, temp file write, fsync, atomic rename, directory fsync)
runs on a background thread via the `Checkpointer` abstraction — no locks
are held during this phase.

Once the snapshot is durable, the WAL lock is re-acquired to truncate:

```rust
let mut wal = event_wal.lock();
wal.truncate_before(processed_seq)?;
```

The truncation rewrites the WAL file (temp + rename + sync), holding the
WAL lock for the full I/O duration.

## Current State

Snapshot I/O has been moved off the lock path via the `Checkpointer`
abstraction (background thread). The remaining contention points are:

1. **Dual lock acquisition** — both state and WAL locks are held
   simultaneously, but only for the duration of a state clone + seq read
   (microseconds at current scale).
2. **WAL truncation** — the WAL lock is held for the full duration of
   `truncate_before()`, which reads the file, writes a filtered temp file,
   fsyncs, and renames.

At current scale (tens of jobs, small step histories), the state clone
completes in microseconds and truncation in low milliseconds. No stalls
have been observed. The checkpoint runs every 60 seconds, so even a brief
stall occurs infrequently.

## When This Matters

This becomes a concern when:

- Hundreds of concurrent jobs with large step histories make the
  state clone expensive (10ms+)
- Large WAL files make truncation slow on rotational storage
- The 60-second interval coincides with a long-running effect chain,
  compounding the stall

## Remaining Fix

Step 3 (snapshot I/O without locks) is done. Two optimizations remain:

**Decouple the initial lock reads** (steps 1–2). Today both locks are
acquired simultaneously. They could be read independently:

```rust
let processed_seq = {
    let wal = event_wal.lock();
    wal.processed_seq()
};
let state_clone = {
    let state = state.lock();
    state.clone()
};
```

The snapshot may be slightly inconsistent (state cloned after WAL seq read,
so it could include one extra event). This is harmless — on recovery, the
WAL replay is idempotent and `apply_event` handles duplicates.

**Move truncation I/O off the lock path.** The WAL lock is held for the
full duration of `truncate_before()` (read → write temp → fsync → rename →
reopen). Consider doing the file rewrite to a temp path without the lock,
then acquiring the lock only for the atomic rename and in-memory state
update.
