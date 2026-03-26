# ACP Runner — PR Split Plan (v2)

> Updated after rebasing `acp-nats-agent` and replacing `RpcServer` + `Runner`
> with `TrogonAgent` (implements `Agent` trait) + `AgentSideNatsConnection`.
> Revised to match repo PR conventions (v3 review).

## Overview

The `acp/runner` branch contains the full implementation of the ACP Runner: the
NATS-backed agent that handles all ACP protocol methods on the runner side.
Rather than merging this as a single large PR, we will split it into one PR per
handler (use case), following the established pattern in this repository.

Each PR:
- Targets `main` directly
- Delivers one complete handler end-to-end
- Includes its own unit and integration tests

PRs 2–13 (handlers) can be merged independently in any order after PR 1.
PRs 14–16 (wire-up) depend on all handler PRs and must merge last.

---

## Architecture

```text
ACP client (Zed / editor)
       ↓ WebSocket / stdio
  acp-nats-ws / acp-nats-stdio  (dumb pipe: ACP ↔ NATS)
       ↓↑ NATS request-reply
  TrogonAgent  (implements Agent trait from agent-client-protocol)
    AgentSideNatsConnection  (from acp-nats-agent)
       ↓
  AgentLoop → Anthropic API (via trogon-secret-proxy)
```

Streaming notifications flow back via `NatsClientProxy` publishing
`SessionNotification` to `{prefix}.{session_id}.client.session.update`.

---

## Conventions

### Title format
```
feat(trogon-acp-runner): add <handler> handler
```
Follow conventional commits. Scope is the crate name. Description is a short
imperative phrase.

### Branch naming
```
feat/acp-runner-<handler>
```
Type prefix + kebab-case. No nested slashes.

### PR body structure
```
## Summary

- <what changed and WHY — architectural decisions, tradeoffs>
- <grouped by concern>

## Not in this slice

- <what was intentionally left out>

## Test plan

- [ ] `cargo test -p <crate>` — N tests pass (M existing + K new)
- [ ] `cargo clippy -p <crate>` — no warnings
- [ ] Coverage ≥ 90%
- [ ] <specific things to verify>
```
`<crate>` is `trogon-acp-runner` for handler PRs (1–13). For wire-up PRs use
the crate being updated: `trogon-acp` (PR 14), `acp-nats-ws` (PR 15),
`acp-nats-stdio` (PR 16).

Coverage threshold: **90%** (enforced by the repo CI gate).

### Size discipline
One handler = one PR. Infrastructure and handler are separate concerns and
should be separate PRs when the infrastructure is non-trivial.

---

## PR Structure

### PR 1 — Foundation + `initialize` handler

**Branch:** `feat/acp-runner-foundation`
**Title:** `feat(trogon-acp-runner): add crate foundation`

> Depends on: `acp-nats-agent` already in `main` (merged as PR #59).

Introduces the `trogon-acp-runner` crate with all shared infrastructure and
the `initialize` handler. `initialize` is included here (not as its own PR)
because without it the binary panics on any connection — the binary must be
minimally functional from PR 1 onward.

- `SessionStore` — NATS JetStream KV-backed session persistence
- `SessionState` — serializable session model
- `ChannelPermissionChecker` — tool permission checking via mpsc channel
- `GatewayConfig` — runtime gateway base URL + token
- `TrogonAgent` struct with `initialize` implemented; all other `Agent` methods
  return a protocol-level "not implemented" error (no `panic!`)
- Binary entry point `main.rs` wired to `AgentSideNatsConnection`

Tests: unit tests for `SessionStore`, `SessionState`, infrastructure
components, and `initialize` handler.

---

### PR 2 — `authenticate` handler

**Branch:** `feat/acp-runner-authenticate`
**Title:** `feat(trogon-acp-runner): add authenticate handler`

Implements `TrogonAgent::authenticate`. Stores gateway config (base URL +
token) for use by subsequent prompts.

Tests: authenticate handler unit tests.

---

### PR 3 — `new_session` handler

**Branch:** `feat/acp-runner-new-session`
**Title:** `feat(trogon-acp-runner): add new_session handler`

Implements `TrogonAgent::new_session`. Creates a new session in `SessionStore`
and builds the initial `SessionState`.

Tests: new_session handler unit and integration tests.

---

### PR 4 — `load_session` handler

**Branch:** `feat/acp-runner-load-session`
**Title:** `feat(trogon-acp-runner): add load_session handler`

Implements `TrogonAgent::load_session`. Loads existing session state from
`SessionStore` and replays history as `SessionNotification` via
`NatsClientProxy`.

Tests: load_session handler unit and integration tests.

---

### PR 5 — `prompt` handler

**Branch:** `feat/acp-runner-prompt`
**Title:** `feat(trogon-acp-runner): add prompt handler`

Implements `TrogonAgent::prompt`. Runs `AgentLoop`, streams `PromptEvent`
responses back to the client as `SessionNotification` via `NatsClientProxy`.
Subscribes to a per-session cancel subject to support interruption. Per-session
semaphore serializes concurrent prompts.

Tests: prompt handler unit and integration tests.

---

### PR 6 — `cancel` handler

**Branch:** `feat/acp-runner-cancel`
**Title:** `feat(trogon-acp-runner): add cancel handler`

Implements `TrogonAgent::cancel`. Publishes an empty message to the per-session
cancel subject, signalling the in-flight prompt to stop.

Tests: cancel handler unit and integration tests.

---

### PR 7 — `set_session_mode` handler

**Branch:** `feat/acp-runner-set-session-mode`
**Title:** `feat(trogon-acp-runner): add set_session_mode handler`

Implements `TrogonAgent::set_session_mode`. Loads session state, updates the
mode, and persists back to `SessionStore`.

Tests: set_session_mode handler unit and integration tests.

---

### PR 8 — `set_session_model` handler

**Branch:** `feat/acp-runner-set-session-model`
**Title:** `feat(trogon-acp-runner): add set_session_model handler`

Implements `TrogonAgent::set_session_model`. Loads session state, updates the
model, and persists back to `SessionStore`.

Tests: set_session_model handler unit and integration tests.

---

### PR 9 — `set_session_config_option` handler

**Branch:** `feat/acp-runner-set-session-config-option`
**Title:** `feat(trogon-acp-runner): add set_session_config_option handler`

Implements `TrogonAgent::set_session_config_option`. Processes configuration
option updates (e.g. mode switches triggered by the `EnterPlanMode` tool).

Tests: set_session_config_option handler unit and integration tests.

---

### PR 10 — `list_sessions` handler

**Branch:** `feat/acp-runner-list-sessions`
**Title:** `feat(trogon-acp-runner): add list_sessions handler`

Implements `TrogonAgent::list_sessions`. Reads all session IDs from
`SessionStore` and returns metadata (id, cwd, title, updated_at) for each.

Tests: list_sessions handler unit and integration tests.

---

### PR 11 — `fork_session` handler

**Branch:** `feat/acp-runner-fork-session`
**Title:** `feat(trogon-acp-runner): add fork_session handler`

Implements `TrogonAgent::fork_session`. Loads the source session state, saves
it under a new UUID, and returns the new session ID.

Tests: fork_session handler unit and integration tests.

---

### PR 12 — `resume_session` handler

**Branch:** `feat/acp-runner-resume-session`
**Title:** `feat(trogon-acp-runner): add resume_session handler`

Implements `TrogonAgent::resume_session`. Validates the session ID exists in
`SessionStore`. Session history is loaded implicitly on the next prompt.

Tests: resume_session handler unit and integration tests.

---

### PR 13 — `close_session` handler

**Branch:** `feat/acp-runner-close-session`
**Title:** `feat(trogon-acp-runner): add close_session handler`

Implements `TrogonAgent::close_session`. Publishes a cancel signal to stop any
in-flight prompt, then removes the session state from `SessionStore`. Also
updates `initialize` to advertise `close: true` in `agentCapabilities`.

Tests: close_session handler unit and integration tests.

---

### PR 14 — Wire-up `trogon-acp`

**Branch:** `feat/acp-runner-wire-trogon-acp`
**Title:** `refactor(trogon-acp): wire TrogonAgent via AgentSideNatsConnection`

> Production binary change — touches `main.rs`, not just test helpers.

Updates `trogon-acp/src/main.rs` to use `TrogonAgent + AgentSideNatsConnection`
instead of the old `Runner`. Removes `Runner` import and replaces the runner
spawn with the `AgentSideNatsConnection` io_task inside the `LocalSet`.

Tests: existing unit and integration tests for `trogon-acp`.

---

### PR 15 — Wire-up `acp-nats-ws`

**Branch:** `feat/acp-runner-wire-acp-nats-ws`
**Title:** `refactor(acp-nats-ws): wire TrogonAgent via AgentSideNatsConnection`

Updates `acp-nats-ws/tests/e2e_runner.rs` test helper to use `TrogonAgent`
instead of the old `RpcServer`. Adds `trogon-acp-runner`, `acp-nats-agent`,
`reqwest`, and `trogon-agent-core` to `[dev-dependencies]` in
`acp-nats-ws/Cargo.toml`.

Tests: `cargo test -p acp-nats-ws --test e2e_runner` — existing e2e tests pass.

---

### PR 16 — Wire-up `acp-nats-stdio`

**Branch:** `feat/acp-runner-wire-acp-nats-stdio`
**Title:** `refactor(acp-nats-stdio): wire TrogonAgent via AgentSideNatsConnection`

Updates `acp-nats-stdio/src/main.rs` e2e test to use `TrogonAgent` instead of
the old `RpcServer`. Adds `trogon-acp-runner`, `acp-nats-agent`, `reqwest`, and
`trogon-agent-core` to `[dev-dependencies]` in `acp-nats-stdio/Cargo.toml`.

Tests: `cargo test -p acp-nats-stdio` — existing e2e tests pass.

---

## Merge Order

PR 1 (foundation) must merge first. PRs 2–13 (handlers) are independent and
can merge in any order after PR 1. PRs 14–16 (wire-up) depend on all handler
PRs and should merge last.

```
PR 1 → PRs 2–13 (any order) → PRs 14–16 (any order)
```

## Notes

- PRs #55, #56, #57 (closed without merging) will be formally closed once this
  plan is executed.
- All code already exists in `acp/runner`. This plan describes how to carve it
  into individual branches.
- CI must pass (lint + coverage gate) on each PR before it is marked ready for
  review.
- `prompt` and `cancel` are separate PRs (5 + 6) following the one-handler
  per-PR convention observed in the merged PR history.
- Wire-up is split into three PRs (14–16), one per binary crate, following the
  pattern of PRs #37 and #38.
