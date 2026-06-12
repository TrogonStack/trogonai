# Scope: a Codex-like permission model for trogon

**Branch:** `feat/scope` (cut from `feat/cli-fixes`)
**Goal:** A low-friction agentic loop. The agent runs freely *inside* a declared
boundary with zero prompts, and is interrupted *only* when it tries to cross
that boundary. Replaces per-action gating ("ask when unsure") with bounded
autonomy ("run free in a box, ask to leave the box").

---

## 1. Design in one paragraph

We do **not** copy Codex's two free dials (`sandbox_mode` × `approval_policy`).
We collapse capability and approval into **one declarative object — a `Scope`**.
The `Scope` says what the agent owns this session (write roots, runnable
commands, network). Inside it: silent auto-allow, no NATS round-trip. Crossing
it: a single behavior, `on_exceed` (escalate once, or deny). Hitting `protected`:
hard deny, never prompt. Approval-timing is **not** a second axis — it is one
property of the boundary that only ever fires on a crossing. You therefore
*cannot* configure the high-friction "wide capability + ask constantly" state;
there is no axis to express it. That is the whole departure from Claude-style
gating.

A compiled-in **baseline** scope ships in the binary, so the model works on
every startup with zero config. TROGON.md / settings.json / per-session values
are optional overrides layered on top.

---

## 2. Where it slots into the existing code

All paths under `rsworkspace/crates/trogon-runner-tools/src/`.

| Concern | Existing anchor | Change |
|---|---|---|
| New types | — | new file `scope.rs` |
| Decision flow | `permission.rs:715` `match self.mode` | insert one scope branch *before* it |
| Hard boundaries | `permission.rs` `explicitly_denied`, `touches_protected_path`, PreToolUse hooks | unchanged; still run **first**, scope can only be stricter |
| Checker struct | `ModePermissionChecker` (permission.rs:479) | add `scope: Option<Scope>` field |
| Builder | `build_mode_permission_checker` (permission.rs:777) | resolve + pass scope |
| Session persistence | `SessionState` (session_store.rs:95) | add `scope: Option<Scope>` (serialized override slot) |
| Reused helpers | `is_read_only_tool`, `is_edit_tool`, `is_read_only_bash_command`, `extract_path_from_input`, `lexical_abs`, `normalize_tool_name` | called by `Scope::evaluate` |

The effective scope is **always concrete** at the checker layer (baseline is the
fallback). `Option<Scope>` on `SessionState` is only the *override* slot.

---

## 3. The types (`scope.rs`)

Follows project conventions: enums at edges, wire/domain split, typed errors,
value objects validated at construction.

```rust
/// What the agent may do this session, as one declarative envelope.
pub struct Scope {
    write: GlobSet,          // globs writable silently (resolved vs cwd)
    run: CommandSet,         // command patterns runnable silently; empty = block bash
    network: NetworkPolicy,  // gates fetch_url, web_search, git_push, gh
    protected: GlobSet,      // additive hard-deny on top of the global protected set
    on_exceed: OnExceed,     // the only surviving knob
}

pub enum NetworkPolicy { Denied, AllowList(Vec<String>), Allowed }  // layers over EgressPolicy
pub enum OnExceed { Escalate, Deny }                                // default Escalate

pub struct GlobSet(/* compiled globset + source strings */);
pub struct CommandSet(/* validated argv match patterns */);

// Wire type — untrusted (TROGON.md / settings.json / NATS), converted once.
pub struct ScopeWire {
    write: Vec<String>, run: Vec<String>,
    network: Option<String>, protected: Vec<String>, on_exceed: Option<String>,
}
pub enum ScopeError { InvalidGlob{..}, UnknownNetwork(String), UnknownOnExceed(String) }
// impl Display + std::error::Error.

impl Scope {
    pub fn baseline(cwd: &str) -> Self;                      // hardcoded, no I/O
    pub fn from_wire(w: ScopeWire, cwd: &str) -> Result<Self, ScopeError>;
    pub fn evaluate(&self, tool: &str, input: &Value, cwd: &str) -> ScopeDecision;
    pub fn on_exceed(&self) -> OnExceed;
}

pub enum ScopeDecision { InScope, OutOfScope, Forbidden }
```

### Classification used by `evaluate`

| Tool class | In-scope rule |
|---|---|
| read-only tools + read-only bash | always `InScope` (read containment handled separately) |
| write/edit (`write_file`, `str_replace`, `multi_edit`, `delete_file`, `notebook_edit`) | resolved path ∈ `write` → `InScope`, else `OutOfScope` |
| `git_commit`, `git_create_branch` | repo (cwd) ∈ `write` → `InScope` (local, reversible) |
| write-bash (non-read-only `bash`) | argv matches `run` → `InScope`, else `OutOfScope` |
| network (`fetch_url`, `web_search`, `git_push`, `gh`) | `network != Denied` (+ host allowed) → `InScope`, else `OutOfScope` |
| any path ∈ `protected` (scope or global) | `Forbidden` |

**Known seam:** write-bash is gated by *static argv match only* — trogon cannot
statically know what a shell command touches. True fs/network containment for
bash needs an OS sandbox (Landlock/seccomp) and is **v2**. For v1 the `run`
allowlist is the gate; the existing WASM containment + `deny_commands` are the
floor.

---

## 4. The integration branch (`permission.rs`)

Inserted in `ModePermissionChecker::check`, after PreToolUse hooks / protected /
explicit-ask (permission.rs:714), before `match self.mode`:

```rust
if let Some(scope) = &self.scope {
    return match scope.evaluate(tool_name, tool_input, cwd) {
        ScopeDecision::Forbidden => { push_audit(.., Denied); false }
        ScopeDecision::InScope   => { push_audit(.., Allowed); true }
        ScopeDecision::OutOfScope => match scope.on_exceed() {
            OnExceed::Escalate => self.inner.inner.check(tool_call_id, tool_name, tool_input).await,
            OnExceed::Deny     => { push_audit(.., Denied); false }
        },
    };
}
// else: existing `match self.mode` runs unchanged
```

`explicitly_denied`, hooks, and global `touches_protected_path` still run first;
scope can only ever be *more* restrictive, never bypass them.

---

## 5. The hardcoded baseline + activation

```rust
impl Scope {
    pub fn baseline(cwd: &str) -> Self {
        Scope {
            write:     GlobSet::single("**", cwd),  // whole repo
            run:       CommandSet::any(),           // floor = deny_commands + WASM sandbox
            network:   NetworkPolicy::Denied,
            protected: GlobSet::empty(),            // global protected set still applies
            on_exceed: OnExceed::Escalate,          // out-of-repo writes / network prompt once
        }
    }
}
```

Resolution in the runner's session setup (always yields a concrete scope):

```rust
let scope = ScopeWire::load_for(cwd)          // TROGON.md / settings.json
    .map(|w| Scope::from_wire(w, cwd))
    .transpose()?
    .unwrap_or_else(|| Scope::baseline(cwd));  // hardcoded fallback
```

**Activation gate (v1): opt-in via env**, mirroring `TROGON_MODE`:

- `TROGON_SCOPE=1` → scope path active (baseline unless overridden).
- unset → legacy `match self.mode` path runs untouched.

This keeps the change zero-impact for other runners/users until dogfooded. Once
proven, flip the default on and drop the gate.

---

## 6. Phasing

**Phase 1 — types + decision (no wiring).** `scope.rs`: all types, `baseline`,
`from_wire`, `evaluate`, `ScopeError`. Full unit-test coverage of `evaluate`
across the classification table. No behavior change anywhere. *Ships green,
inert.*

**Phase 2 — wire into the checker.** Add `scope` field to `ModePermissionChecker`
+ `SessionState`; resolve in `build_mode_permission_checker`; insert the
`permission.rs` branch; env-gate on `TROGON_SCOPE`. Unit-test the branch
(InScope silent, OutOfScope→escalate, OutOfScope+Deny, Forbidden, protected-still-
wins, deny-rule-still-wins).

**Phase 3 — static loading.** `ScopeWire::load_for` parses a `[scope]` block from
TROGON.md (like `PermissionRules::parse`, skipping fenced code) and/or
settings.json. Tests for parse + invalid-glob error + precedence
(override beats baseline).

**Phase 4 — dogfood.** Set `TROGON_SCOPE=1` in `.env.local`, run the acp runner
locally, confirm: edits in-repo are silent, a write to `/tmp` prompts once, a
`curl` prompts once, `.env` stays hard-denied. Tune the baseline from real use.

**Phase 5 (later) — default-on + drop gate**, once Phase 4 is convincing.

### Out of v1, explicitly (v2 backlog)
- OS-level bash containment (Landlock + seccomp) for real fs/network enforcement.
- Adaptive / self-widening scope (a moving trust boundary — deliberately deferred).
- **Codex runner pass-through:** translate `Scope` → `codex app-server` sandbox
  config. The subprocess owns execution (we only observe), so the only way it
  gets scope is native enforcement via app-server config. The other three
  runners (acp/xai/openrouter) dispatch in-process and use `evaluate` directly.

---

## 7. Testing

- `scope.rs` unit tests: every row of the classification table, both `on_exceed`
  values, `protected` overlap, glob resolution vs cwd, `from_wire` happy + each
  `ScopeError`.
- `permission.rs` tests: the new branch, plus regressions proving hooks /
  explicit-deny / global-protected / explicit-ask still pre-empt scope.
- `session_store.rs`: `Scope` round-trips through serde (override slot).
- Gate-off proof: `TROGON_SCOPE` unset → identical behavior to today (the legacy
  mode tests must still pass unchanged).

Run: `cargo test -p trogon-runner-tools`, then `cargo clippy` (workspace is
warnings-as-errors). Serialize cargo against the shared target.

---

## 8. Rollout / commit hygiene

- Stage only `scope.rs`, `permission.rs`, `session_store.rs` (+ runner setup) per
  commit. **Do not** sweep in the unrelated uncommitted `trogon-agent/src/tools/mod.rs`
  edit carried onto this branch.
- No `cargo fmt -p` on the whole crate (rewrap churn).
- No auto-push; ask first.
- One commit per phase keeps the inert-vs-wired boundary reviewable.
