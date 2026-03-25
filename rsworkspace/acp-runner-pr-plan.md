# ACP Runner — PR Split Plan

## Overview

The `acp/runner` branch contains the full implementation of the ACP Runner: the NATS-backed agent that handles all ACP protocol methods on the runner side. Rather than merging this as a single large PR, we will split it into one PR per handler (use case), following the established pattern in this repository.

Each PR:
- Targets `main` directly
- Delivers one complete handler end-to-end (bridge + runner)
- Includes its own unit and integration tests
- Can be merged independently, in any order

---

## PR Structure

### PR 1 — Foundation + `initialize` handler

**Branch:** `acp/runner/initialize`

Introduces the `trogon-acp-runner` crate and all infrastructure required by subsequent handlers:

- `SessionStore` — NATS JetStream KV-backed session persistence
- `PermissionGate` — tool permission checking
- `PromptConverter` — converts runner `PromptEvent` stream to ACP `SessionNotification`
- `RpcServer` — NATS request-reply dispatcher skeleton
- `Runner` — prompt/streaming entry point
- Binary entry point (`main.rs`)
- `initialize` handler — returns agent capabilities, auth methods, session modes and models

Tests: unit tests for all infrastructure components + `initialize` handler tests.

---

### PR 2 — `authenticate` handler

**Branch:** `acp/runner/authenticate`

Adds the `authenticate` handler to `RpcServer`. Returns an empty `AuthenticateResponse` (no authentication required — gateway config is passed via environment variables).

Tests: authenticate handler unit tests.

---

### PR 3 — `new_session` handler

**Branch:** `acp/runner/new_session`

Adds the `new_session` handler. Creates a new session in `SessionStore`, builds the initial `SessionState`, and publishes `session.ready` to NATS after replying.

Tests: new_session handler unit and integration tests.

---

### PR 4 — `load_session` handler

**Branch:** `acp/runner/load_session`

Adds the `load_session` handler. Loads existing session state from `SessionStore` and publishes `session.ready` after replying.

Tests: load_session handler unit and integration tests.

---

### PR 5 — `prompt` handler

**Branch:** `acp/runner/prompt`

Adds the `prompt` handler. Runs the agent loop, streams `PromptEvent` responses back to the bridge via NATS pub/sub, and converts them to ACP `SessionNotification` using `PromptConverter`.

Tests: prompt handler unit and integration tests.

---

### PR 6 — `cancel` handler

**Branch:** `acp/runner/cancel`

Adds the `cancel` handler. Signals the in-flight prompt to stop via a cancellation channel.

Tests: cancel handler unit and integration tests.

---

### PR 7 — `set_session_mode` handler

**Branch:** `acp/runner/set_session_mode`

Adds the `set_session_mode` handler. Loads session state, updates the mode, and persists back to `SessionStore`.

Tests: set_session_mode handler unit and integration tests.

---

### PR 8 — `set_session_model` handler

**Branch:** `acp/runner/set_session_model`

Adds the `set_session_model` handler. Loads session state, updates the model, and persists back to `SessionStore`.

Tests: set_session_model handler unit and integration tests.

---

### PR 9 — `set_session_config_option` handler

**Branch:** `acp/runner/set_session_config_option`

Adds the `set_session_config_option` handler. Processes configuration option updates (e.g. mode switches triggered by the `EnterPlanMode` tool).

Tests: set_session_config_option handler unit and integration tests.

---

### PR 10 — `list_sessions` handler

**Branch:** `acp/runner/list_sessions`

Adds the `list_sessions` handler. Reads all session IDs from `SessionStore` and returns metadata (id, cwd, title, updated_at) for each.

Tests: list_sessions handler unit and integration tests.

---

### PR 11 — `fork_session` handler

**Branch:** `acp/runner/fork_session`

Adds the `fork_session` handler. Loads the source session state, saves it under a new UUID, and publishes `session.ready` for the new session.

Tests: fork_session handler unit and integration tests.

---

### PR 12 — `resume_session` handler

**Branch:** `acp/runner/resume_session`

Adds the `resume_session` handler. Validates the session ID and publishes `session.ready`. Session history is loaded implicitly on the next prompt.

Tests: resume_session handler unit and integration tests.

---

### PR 13 — `close_session` handler

**Branch:** `acp/runner/close_session`

Adds the `close_session` handler. Removes the session state from `SessionStore`.

Tests: close_session handler unit and integration tests.

---

## Merge Order

All PRs target `main` and are independent. They can be reviewed in any order. Suggested merge sequence follows the list above (PR 1 → PR 13) to build incrementally, but no PR has a hard dependency on another.

## Notes

- The existing PRs #55 (`acp/foundation`), #56 (`acp/bridge`), and #57 (`acp/runner`) will be closed without merging once this plan is executed.
- All code already exists in `acp/runner`. This plan describes how to carve it into individual branches.
- CI must pass (lint + coverage gate) on each PR before it is marked ready for review.
