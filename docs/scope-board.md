# Scope permission model — task board

Board-fill source of truth for the Codex-like permission model (`Scope`).
Design + implementation plan: `rsworkspace/docs/scope-permission-plan.md`.
Branch: `feat/scope` (cut from `feat/cli-fixes`).

**Totals: 18 tasks — 15 to ship v1, plus 3 in the v2 backlog.**

| Phase | Tasks | IDs |
|-------|-------|-----|
| Phase 1 — types + decision logic (inert) | 5 | SCOPE-1…5 |
| Phase 2 — wire into live permission path | 5 | SCOPE-6…10 |
| Phase 3 — static config loading | 3 | SCOPE-11…13 |
| Phase 4 — dogfood | 1 | SCOPE-14 |
| Phase 5 — promote (deferred) | 1 | SCOPE-15 |
| v2 backlog (separate effort) | 3 | SCOPE-V2-1/2/3 |

**Critical path:** SCOPE-1 → 4 → 5 (Phase 1 is the bulk and the only part with
real logic). Everything after is wiring + config.

---

## Phase 1 — types + decision logic (inert, no behavior change)

### SCOPE-1 — `scope.rs` skeleton + type defs
- **Priority:** P0 · **Depends:** —
- **Acceptance:** `Scope`, `NetworkPolicy`, `OnExceed`, `ScopeDecision`,
  `GlobSet`, `CommandSet`, `ScopeError` defined; `mod scope;` in `lib.rs`;
  compiles. Glob matcher reuses the one `permission_rules.rs` already uses (no
  new dependency).

### SCOPE-2 — `Scope::baseline(cwd)`
- **Priority:** P0 · **Depends:** SCOPE-1
- **Acceptance:** Returns write `**`, run any, network `Denied`, on_exceed
  `Escalate`, protected empty. Pure, no I/O.

### SCOPE-3 — `ScopeWire` + `from_wire`
- **Priority:** P0 · **Depends:** SCOPE-1
- **Acceptance:** serde wire type; converts once; each bad input maps to the
  right `ScopeError` variant (`InvalidGlob` / `UnknownNetwork` / `UnknownOnExceed`).

### SCOPE-4 — `Scope::evaluate()`
- **Priority:** P0 · **Depends:** SCOPE-1
- **Acceptance:** Full classification table (read-only / edit / git-write /
  write-bash / network / protected / unknown→fail-closed), reusing
  `is_read_only_tool`, `is_edit_tool`, `is_read_only_bash_command`,
  `extract_path_from_input`, `lexical_abs`, `normalize_tool_name`.

### SCOPE-5 — Phase-1 unit tests
- **Priority:** P0 · **Depends:** SCOPE-2, SCOPE-3, SCOPE-4
- **Acceptance:** One test per table row; both `on_exceed` values; `from_wire`
  happy path + each error; cwd-relative glob resolution;
  `cargo test -p trogon-runner-tools` + `cargo clippy` green. **Zero behavior
  change** — nothing calls `evaluate` yet.

---

## Phase 2 — wire into the live permission path

### SCOPE-6 — `scope` field on checker + `SessionState`
- **Priority:** P0 · **Depends:** Phase 1
- **Acceptance:** `Option<Scope>` added to `ModePermissionChecker` and
  `SessionState`; serde round-trips through `session_store`.

### SCOPE-7 — Resolve effective scope in builder
- **Priority:** P0 · **Depends:** SCOPE-6
- **Acceptance:** `build_mode_permission_checker` yields a concrete scope
  (override → `baseline` fallback; never `None` at the checker layer).

### SCOPE-8 — Insert scope branch in `check`
- **Priority:** P0 · **Depends:** SCOPE-7
- **Acceptance:** Branch at `permission.rs:715`, before `match self.mode`:
  InScope→silent allow, OutOfScope→`on_exceed`, Forbidden→deny. Runs *after*
  explicitly_denied / hooks / global protected / explicit-ask.

### SCOPE-9 — `TROGON_SCOPE=1` activation gate
- **Priority:** P0 · **Depends:** SCOPE-8
- **Acceptance:** Unset → legacy `match self.mode` path runs unchanged; set →
  scope path active (baseline unless overridden). Mirrors `TROGON_MODE`.

### SCOPE-10 — Phase-2 tests + regressions
- **Priority:** P0 · **Depends:** SCOPE-8, SCOPE-9
- **Acceptance:** Branch behavior covered (InScope/OutOfScope→escalate /
  OutOfScope+Deny / Forbidden); proves deny-rules, hooks, global-protected, and
  explicit-ask still pre-empt scope; **gate-off → all existing mode tests pass
  unchanged.**

---

## Phase 3 — static config loading

### SCOPE-11 — Parse `[scope]` from TROGON.md
- **Priority:** P1 · **Depends:** Phase 2
- **Acceptance:** `ScopeWire::load_for` parses a `[scope]` block from TROGON.md,
  parsed like `PermissionRules::parse` (skips fenced code blocks).

### SCOPE-12 — settings.json `[scope]` + precedence
- **Priority:** P1 · **Depends:** SCOPE-11
- **Acceptance:** Precedence enforced: session > settings.json / TROGON.md >
  compiled baseline.

### SCOPE-13 — Loading / precedence tests
- **Priority:** P1 · **Depends:** SCOPE-11, SCOPE-12
- **Acceptance:** Parse, invalid-glob error surfaced, override-beats-baseline.

---

## Phase 4 — dogfood

### SCOPE-14 — Local dogfood on acp runner
- **Priority:** P1 · **Depends:** Phase 3
- **Acceptance:** With `TROGON_SCOPE=1`: in-repo edit silent, `/tmp` write
  prompts once, `curl` prompts once, `.env` hard-denied. Baseline tuned from
  real use.

---

## Phase 5 — promote (deferred)

### SCOPE-15 — Default-on + drop env gate
- **Priority:** P2 · **Depends:** SCOPE-14 passing
- **Acceptance:** Baseline active without the flag; `TROGON_SCOPE` gate removed.

---

## v2 backlog (separate effort, NOT v1)

### SCOPE-V2-1 — OS sandbox for bash (Landlock + seccomp)
- **Priority:** P2
- **Notes:** Real filesystem/network containment; closes the static-argv seam
  where trogon can't know what a shell command touches. Multi-day.

### SCOPE-V2-2 — Codex-runner scope pass-through
- **Priority:** P2
- **Notes:** Translate `Scope` → `codex app-server` sandbox config. The
  subprocess owns execution (trogon only observes), so it self-enforces.

### SCOPE-V2-3 — Adaptive / self-widening scope
- **Priority:** P3
- **Notes:** Deliberately deferred — a moving trust boundary is the kind of
  thing that bites once, badly. Static scope drawn generously is the v1 default.
