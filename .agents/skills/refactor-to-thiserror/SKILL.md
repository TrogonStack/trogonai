---
name: refactor-to-thiserror
description: Find Rust error types with hand-written Display/Error/From impls and refactor them to derive thiserror::Error, preserving behavior.
allowed-tools:
  - Bash
  - Read
  - Edit
  - Grep
---

Refactor manual Rust error boilerplate to `#[derive(thiserror::Error)]`.

1. Find candidates (from `rsworkspace/`):
   ```bash
   grep -rln "impl std::error::Error\|impl fmt::Display for\|impl std::fmt::Display for" --include="*.rs" crates
   ```

2. Refactor each error type to the codebase idiom (see `crates/trogon-decider-runtime/src/snapshot/codec/snapshot_decode_error.rs`):
   - `#[derive(Debug, thiserror::Error)]` enum; keep existing derives.
   - One `#[error("...")]` per variant — copy the old `Display` text verbatim so `to_string()` is unchanged.
   - `#[source]` for wrapped errors; `#[from]` to replace an `impl From`; `#[error(transparent)]` for pass-throughs.
   - Delete the now-redundant `impl Display`, `impl std::error::Error`, and replaced `impl From` blocks.
   - A struct that `match`es over a field → convert to an enum (one variant per arm).

3. Skip when the derive would change behavior: dynamic `Display`, custom `Error` methods beyond `source()`, non-error `Display` (value objects), or any public API / exact message text other code depends on. When in doubt, leave it.

4. If missing, add to the crate's `Cargo.toml`: `thiserror = { workspace = true }`.

5. Verify: `cargo check -p <crate> && cargo test -p <crate>`. Tests assert exact messages — fix the derive, never the test.
