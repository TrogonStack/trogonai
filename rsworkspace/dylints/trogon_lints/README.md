# trogon_lints

Repository-owned Rust policy lints for Trogon.

This Dylint crate is intentionally isolated from the parent Cargo workspace and
pins its compiler in `rust-toolchain.toml`. The nightly toolchain is only for
building the rustc-integrated lint library; the main Rust workspace keeps using
its normal toolchain.

## Rules

- `error_string_comparison`: prevents semantic checks against strings derived
  from `std::error::Error::to_string`.
- `manual_error_impl`: requires deriving `std::error::Error` with `thiserror`
  instead of hand-writing the impl.
- `inline_module_block`: requires modules to live in their own file
  (`mod foo;`) instead of inline blocks (`mod foo { ... }`). Macro-generated
  modules are exempt; suppress a justified exception with
  `#[allow(inline_module_block)]` at the site.

## Run

From `rsworkspace/`:

```bash
env -u RUSTUP_TOOLCHAIN DYLINT_RUSTFLAGS='-Derror-string-comparison -Dinline-module-block -Dmanual-error-impl' cargo dylint --path dylints/trogon_lints --workspace --no-deps -- --all-features
```

To audit existing test-target debt before enabling the stricter gate, add
`--all-targets` after `--all-features`:

```bash
env -u RUSTUP_TOOLCHAIN DYLINT_RUSTFLAGS='-Derror-string-comparison -Dinline-module-block -Dmanual-error-impl' cargo dylint --path dylints/trogon_lints --workspace --no-deps -- --all-features --all-targets
```
