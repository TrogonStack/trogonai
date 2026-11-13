# Hydro verification

Status: experimental

This development-only Cargo project explores scheduler recovery schedules with
[Hydro](https://hydro.run/). It has an independent dependency lock and Rust
toolchain because Hydro's staged simulator compilation uses a newer compiler
than the production workspace. Production crates do not depend on this project.

From the repository root:

```sh
mise run verification:hydro
```

Install the pinned compiler with `mise install rust@1.98.1` if needed. The first
run downloads and compiles Hydro plus its simulator-generated dynamic libraries.
Later runs reuse Cargo's normal build cache. The task is opt-in and is not part
of required CI or the default production build.

Read [Verify scheduler recovery with Hydro](../../docs/how-to/verify-scheduler-with-hydro.md)
for the modeled properties, bounds, source correspondence, and negative controls.
