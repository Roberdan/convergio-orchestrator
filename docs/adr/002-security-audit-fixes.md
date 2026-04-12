# ADR-002: Security Audit Fixes

**Status:** Accepted  
**Date:** 2025-07-24  
**Author:** Security Audit (Copilot)

## Context

A comprehensive security audit of convergio-orchestrator (20,378 LOC) identified
several vulnerabilities across path traversal, state machine integrity, input
validation, and credential management.

## Findings & Fixes

### 1. Path Traversal in Artifact Upload/Download (HIGH)

**artifact_routes.rs** — User-supplied filenames were joined directly to the
artifacts directory without sanitization. A filename like `../../etc/passwd`
could escape the artifacts root.

**Fix:** Added `sanitize_filename()` that strips path separators, `..`
components, and non-safe characters. Added `starts_with()` defense-in-depth
check on resolved path before write/read.

### 2. Path Traversal in File Size Scanner (MEDIUM)

**file_size_scan.rs** — The `/api/lint/file-size-check` endpoint accepted
arbitrary absolute paths, allowing scanning of any directory on the host.

**Fix:** Reject absolute paths and paths containing `..` components.

### 3. Hardcoded Auth Fallback Token (MEDIUM)

**task_instructions.rs** — The KB enrichment endpoint fell back to
`bearer_auth("dev-local")` when `CONVERGIO_AUTH_TOKEN` was unset. The generated
agent curl instructions also embedded `dev-local` as a fallback.

**Fix:** Removed fallback — requests proceed without auth header if env var is
unset (fail-open for local dev, but no leaked credential). Changed curl template
to use `${CONVERGIO_AUTH_TOKEN:?must be set}` which errors explicitly.

### 4. Wave Status Bypass (HIGH)

**plan_routes_ext.rs** — `handle_wave_update` accepted any arbitrary string as
wave status with no validation.

**Fix:** Added whitelist of valid wave statuses:
`pending | in_progress | done | cancelled | failed | paused`.

### 5. Task Status Catch-All Gate Bypass (HIGH)

**gates.rs** — The `check_task_transition` function had a `_ => Ok(())` arm
that silently accepted any unknown status, bypassing all lifecycle gates.

**Fix:** Replaced with explicit whitelist of non-gated statuses
(`pending | cancelled | skipped | stale | failed`) and an error for unknown
values.

### 6. Input Validation Gaps (MEDIUM)

**plan_routes.rs / task_routes.rs** — No length bounds on user-supplied strings
(plan name, description, project_id, etc.).

**Fix:** Added maximum length checks on all string fields in `handle_create`
(plans) and `handle_task_create` (tasks).

## Not Fixed (Accepted Risk)

- **No auth middleware on routes:** The orchestrator runs as a local daemon
  behind the Convergio gateway. Auth is handled at the gateway/IPC layer.
  Adding per-route auth would break the local agent protocol. Documented as
  accepted architectural decision.

- **`unsafe` block in reaper.rs:** `libc::kill()` for orphan process cleanup is
  a controlled, well-audited use of unsafe. The PID is parsed from `ps` output
  (integer only, no injection possible).

- **TOCTOU in completion flows:** SQLite's default serialized mode + single-writer
  WAL journaling provides sufficient atomicity for the current deployment model
  (single daemon process). Full SQL transaction wrapping would add complexity
  with minimal benefit.

## Consequences

- Path traversal attacks on artifact upload/download are blocked
- State machine integrity is enforced for both task and wave statuses
- No hardcoded credentials ship in generated instructions
- Input validation prevents oversized payloads
