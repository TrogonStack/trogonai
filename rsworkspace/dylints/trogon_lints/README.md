# trogon_lints

Repository-owned Rust policy lints for Trogon.

This Dylint crate is intentionally isolated from the parent Cargo workspace and
pins its compiler in `rust-toolchain.toml`. The nightly toolchain is only for
building the rustc-integrated lint library; the main Rust workspace keeps using
its normal toolchain.

## Rules

- `error_string_comparison`: prevents semantic checks against strings derived
  from `std::error::Error::to_string`.

## Run

From `rsworkspace/`:

```bash
env -u RUSTUP_TOOLCHAIN DYLINT_RUSTFLAGS='-Derror-string-comparison' cargo dylint --path dylints/trogon_lints --workspace --no-deps -- --all-features
```

To audit existing test-target debt before enabling the stricter gate, add
`--all-targets` after `--all-features`:

```bash
env -u RUSTUP_TOOLCHAIN DYLINT_RUSTFLAGS='-Derror-string-comparison' cargo dylint --path dylints/trogon_lints --workspace --no-deps -- --all-features --all-targets
```
