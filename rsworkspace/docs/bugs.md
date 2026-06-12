# Bugs — trogon-cli

Confirmed by reading the source on branch `platform` (file:line references below).
Scope: terminal CLI (`crates/trogon-cli`). Each entry was verified directly in code,
not inferred. Fixes listed are the verified-correct direction, not yet applied.

---

## BUG-CLI-1 — `--print` reports success (exit 0) when the stream dies without `Done`
**Severity: HIGH** (silently turns a failure into a success for scripts/CI)

`crates/trogon-cli/src/print.rs`
- `stop_reason` is initialized to `"end_turn"` (≈ line 85) and only overwritten on a
  `StreamEvent::Done(reason)` or `StreamEvent::Error`.
- The loop is `while let Some(event) = rx.recv().await { ... }`. If the channel closes
  with no `Done` event (runner crash, NATS disconnect, dropped notification), the loop
  ends, the code falls through to emit `stop_reason = "end_turn"`, and
  `PrintExitCode::from_stop_reason("end_turn")` returns **exit 0**.

**Impact:** `trogon --print "..." && deploy` treats a crashed/truncated turn as success.

**Fix:** distinguish "stream ended without a terminal event" from a real `end_turn`.
Track whether a `Done`/`Error` was seen; if the stream closes without one, emit a
non-success `stop_reason` (e.g. `"error"`/`"incomplete"`) and a non-zero exit code.

---

## BUG-CLI-2 — `/init` writes `TROGON.md` to the git root, not the current directory
**Severity: MED** (writes/overwrites a file in an unexpected location)

`crates/trogon-cli/src/repl.rs:1043-1044`
```rust
let root = find_git_root(&cwd).unwrap_or_else(|| cwd.clone());
let dest = root.join("TROGON.md");
```
`find_git_root` walks **up** to the first `.git`. Running `/init` from a subdirectory of
a monorepo writes to the **repo root**, not the directory the user is in. The analysis
prompt is also seeded from the root listing. `--force` overwriting then targets the
root file.

**Fix:** write to the session `cwd` (or prompt/confirm the destination). If the git-root
behavior is intentional, surface the resolved path before writing.

---

## BUG-CLI-3 — Ctrl+C cancel is fire-and-forget; no confirmation the turn stopped
**Severity: MED**

`crates/trogon-cli/src/session.rs` (`cancel`)
- `cancel()` publishes to `{prefix}.session.{id}.agent.cancel` with no reply subject and
  no await of an ack — it returns as soon as the publish is flushed.
- The Ctrl+C arm in `repl.rs` breaks the stream loop immediately after `cancel().await`
  returns and drops the receiver.

**Impact:** if the publish is lost (NATS disconnect window), the runner keeps working —
burning tokens / running tools — while the REPL has returned to the prompt and the user
believes the turn was cancelled. No retry, no user-visible confirmation.

**Fix:** request/reply (or observe a cancellation ack / terminal event) so the user gets
confirmation the runner actually stopped; surface a warning if no ack arrives.

---

## BUG-CLI-4 — `process::exit` / signal termination bypasses `KillOnDrop`, orphaning `nats-server`
**Severity: MED** (partial — some paths are mitigated)

`crates/trogon-cli/src/lib.rs:39-160` (`KillOnDrop`, autostarted child) and
`crates/trogon-cli/src/main.rs:226` (`std::process::exit(...)`).
- The autostarted `nats-server` child is killed via the `KillOnDrop` guard's `Drop`.
- `std::process::exit` does **not** run `Drop`; neither does signal-based termination.
- Some paths explicitly `drop()` the guard before exiting (MED-40, `main.rs:304`), but a
  second Ctrl+C during the inter-turn window (when no `ctrl_c()` future is armed) hits the
  default SIGINT handler → the process dies → `Drop` never runs → the `nats-server` child
  is leaked.

**Fix:** install a signal handler that performs cleanup (kill the child) before exiting,
or ensure the autostarted server is reaped on any exit path (e.g. process group / setsid
+ kill on signal).

---

## Notes
- All four are deterministic local-code behaviors, verified by reading the source.
- Not covered by CI: `trogon-cli` integration tests are not run in the CI workflows
  (CI runs `cargo test --workspace --lib` + a few named suites), which is why these
  drifted unnoticed.
