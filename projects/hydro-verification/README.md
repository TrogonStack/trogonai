# Hydro verification

Status: experimental, local only

This development-only project uses [Hydro](https://hydro.run/) to enumerate finite
client command orderings and test the actual scheduler domain deciders for each
order. The harness depends on the production domain crate through Cargo.

From the repository root:

```sh
mise run verification:hydro
```

Or explicitly launch it through Bazel:

```sh
mise exec bazel -- bazel run //projects/hydro-verification:verify
```

Install the pinned compiler with `mise install rust@1.98.1` if needed. The first
run compiles Hydro and its generated simulation libraries. The project has its
own Cargo lock and compiler pin because Hydro requires a newer Rust compiler
than the production workspace. Production crates do not depend on this project.
Both entrypoints refuse to run when `CI` or `GITHUB_ACTIONS` is nonempty.
The Bazel target is tagged `manual`, so wildcard builds and tests omit it.
Bazel stages a local launcher for the same mise/Cargo suite; the Rust tests and
Hydro-generated libraries are built by Cargo rather than native Bazel actions.

Read [Verify scheduler decisions with Hydro](../../docs/how-to/verify-scheduler-with-hydro.md)
for the actual implementation calls, assertions, and finite scenario bounds.
