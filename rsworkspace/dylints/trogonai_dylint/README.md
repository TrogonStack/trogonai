# trogonai_dylint

Rust policy lints owned by this repository, ahead of any move to the shared
library.

The rules this crate used to carry now live in
[`trogon_dylint_lints`](https://github.com/TrogonStack/rusty-monorepo/tree/main/crates/trogon_dylint_lints),
which `rsworkspace/Cargo.toml` names as a `[workspace.metadata.dylint]` entry.
This crate stays behind as the place a new rule is written first, where it can
be iterated on against this repository's own code before it is proposed to a
library other workspaces depend on.

Both libraries load into the same rustc lint store, so a name declared here must
not also be declared in the shared library. Moving a rule out means deleting it
here in the same change that adds it there.

This crate is deliberately outside the parent Cargo workspace and pins its
compiler in `rust-toolchain.toml`. The nightly toolchain builds the
rustc-integrated lint library only; the main Rust workspace keeps its own
toolchain. That pin must stay equal to the shared library's, since one rustc
driver loads both.

## Develop

```bash
cd rsworkspace/dylints/trogonai_dylint
rustup run "$(python -c 'import tomllib; print(tomllib.load(open("rust-toolchain.toml","rb"))["toolchain"]["channel"])')" cargo test
```

Declare the lint in `src/lib.rs`, register it in `register_lints`, and give it
fixtures under `ui/` (dependency-free cases) or `examples/` (cases needing real
`tracing`/`opentelemetry` types).
