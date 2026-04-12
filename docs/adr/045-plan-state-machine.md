# ADR-045: Plan State Machine (FSM)

**Status**: Accepted
**Date**: 2026-04-11
**Author**: Roberto + Copilot

## Context

Plan status transitions were stringly-typed (`UPDATE plans SET status = ?`)
with no validation. Any route handler could set any status, leading to:

- Impossible states (e.g., `todo → done` bypassing execution)
- Silent deadlocks from invalid transitions (#666, #734)
- `force_resume` as the only recovery, which itself created new issues
- No compile-time or runtime guarantees on lifecycle correctness

The architecture v2 doc proposed a full rewrite with formal state machines.
We rejected the rewrite and implemented a surgical fix instead.

## Decision

Introduce `plan_state.rs` with a typed `PlanStatus` enum and explicit
transition validation:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanStatus {
    Todo, Draft, Approved, InProgress,
    Paused, Stale, Failed, Done, Cancelled,
}

impl PlanStatus {
    pub fn can_transition_to(&self, target: &PlanStatus) -> bool { ... }
}
```

### Allowed transitions (22 total)

```
Todo → Draft, Approved, InProgress, Cancelled
Draft → Approved, Cancelled
Approved → InProgress, Cancelled
InProgress → Paused, Failed, Done, Stale, Cancelled
Paused → InProgress, Cancelled, Failed
Stale → InProgress, Cancelled, Failed
Failed → Draft, Cancelled
Done → (terminal)
Cancelled → (terminal)
```

### Wiring

`set_plan_status()` in `plan_routes.rs` reads current status from DB,
calls `validate_plan_transition(current, target)`, and rejects invalid
transitions with a clear error message including current and requested status.

Force-ops (`plan_force_ops.rs`) bypass the FSM by design — they write
directly to the DB as admin escape hatches.

## Consequences

- **Breaking**: any code path that relied on `todo → done` or other
  impossible transitions will now get an error response instead of silently
  succeeding. This is intentional.
- **Tests updated**: E2E lifecycle tests now set plan to `in_progress`
  before completing, matching real-world flow.
- **12 unit tests** cover normal flow, pause/resume, stale recovery,
  terminal blocking, cancel-from-anywhere, and parse roundtrip.

## Alternatives Rejected

- **Full orchestrator rewrite** (architecture v2 doc): too much scope,
  months of work, blocks feature development.
- **Typestate pattern** (`Plan<Todo>` → `Plan<Approved>`): elegant but
  requires rewriting every route handler signature. Overkill for DB-backed
  state stored as strings.
