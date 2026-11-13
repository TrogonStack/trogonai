# Hydro verification

This development-only project uses [Hydro](https://hydro.run/) to enumerate finite
client command orderings and test the actual scheduler domain deciders for each
order. The harness depends on the production domain crate through Cargo.

From the repository root:

```sh
mise run verification:hydro
```

Install the pinned compiler with `mise install rust@1.98.1` if needed. The first
run compiles Hydro and its generated simulation libraries. The project has its
own Cargo lock and compiler pin because Hydro requires a newer Rust compiler
than the production workspace. Production crates do not depend on this project.
The task is opt-in and is not part of required CI.

Read [Verify scheduler decisions with Hydro](../../docs/how-to/verify-scheduler-with-hydro.md)
for the actual implementation calls, assertions, and finite scenario bounds.
