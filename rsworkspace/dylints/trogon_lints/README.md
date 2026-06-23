# trogon_lints

Repository-owned Rust policy lints for Trogon.

This Dylint crate is intentionally isolated from the parent Cargo workspace and
pins its compiler in `rust-toolchain.toml`. The nightly toolchain is only for
building the rustc-integrated lint library; the main Rust workspace keeps using
its normal toolchain.

## Rules

Each rule's default level is declared in `src/lib.rs`, so policy lives in the
lint crate rather than in per-invocation flags.

- `error_string_comparison` (`deny`): prevents semantic checks against strings
  derived from `std::error::Error::to_string`.
- `manual_error_impl` (`deny`): requires deriving `std::error::Error` with
  `thiserror` instead of hand-writing the impl.
- `inline_module_block` (`deny`): requires modules to live in their own file
  (`mod foo;`) instead of inline blocks (`mod foo { ... }`). Macro-generated
  modules and `@generated` files (proto codegen, etc.) are exempt; suppress a
  justified exception with
  `#[cfg_attr(dylint_lib = "trogon_lints", allow(inline_module_block))]` at the
  site. As a late (HIR) pass it sees `#[cfg(test)] mod tests { ... }` only when
  the test target is compiled, i.e. when linting with `--all-targets`.

## Run

From `rsworkspace/` (the `deny` rules are enforced by their declared default
level, no flags needed). This mirrors CI:

```bash
env -u RUSTUP_TOOLCHAIN cargo dylint --path dylints/trogon_lints --workspace --no-deps -- --all-features
```

Add `--all-targets` to also lint test targets such as
`#[cfg(test)] mod tests { ... }`, which a late (HIR) pass only sees when the
test target is compiled:

```bash
env -u RUSTUP_TOOLCHAIN cargo dylint --path dylints/trogon_lints --workspace --no-deps -- --all-features --all-targets
```
