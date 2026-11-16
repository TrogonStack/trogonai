# Build and test with Bazel

Bazel is the repository's CI. The `CI Bazel` workflow builds, lints, and tests
every target with `bazel coverage //...`, and its `Bazel result` check covers the
Rust, Go, protobuf, TypeScript, documentation, ADR, workload identity, and
workflow checks that used to run in one workflow per language.

## What runs outside Bazel

A few checks cannot run inside Bazel, so each keeps a small workflow of its own:

| Workflow | Check | Why it is not a Bazel target |
| --- | --- | --- |
| `CI Dylint` | Repository Rust policy lints and their tests | The lint library links `rustc_private` crates from a pinned nightly with `rustc-dev`, which the Bazel Rust toolchain lacks. |
| `CI Cargo` | Rust tests tagged `manual` or skipped in a `BUILD.bazel` | They shell out to Cargo, load components built by `cargo component`, or read files outside their Bazel package. |
| `CI Buf Breaking` | `buf breaking` against the base branch | It compares against another revision of the repository. |
| `CI pnpm Lockfiles` | `pnpm install --frozen-lockfile` for `docs` and `tsworkspace` | Bazel reads the lockfiles; it does not check them against each `package.json`. |
| `CI Automation` | CI selection tests, the canary helper tests, and `bash -n` over the mise tasks | `.bazelignore` leaves `.config/mise/tasks` out of Bazel. |
| `CI Bazel Fork Coverage` | Independently compare a fork's report and publish its PR comment | Publishing needs a trusted default-branch workflow with separate token permissions. |

Fork coverage comparison also gates `Bazel result` using a read-only job in
`CI Bazel`. See the [fork coverage contract](../reference/bazel-fork-coverage.md)
for policy, provenance, publishing boundaries and stale-run handling.

`CI Cargo` reads the `manual` tags and `--skip` arguments from each crate's
`BUILD.bazel`, so tagging a test `manual` moves it there with no list to update.
CodeQL, release, PR title, and Pages workflows are unrelated to the build and
stay as they are.

## Run the native targets

Install the pinned build tools, then invoke the repository tasks:

```sh
mise run --skip-tools bazel:setup
mise run --skip-tools bazel:build
mise run --skip-tools bazel:test
```

`.config/mise/config.toml` is the version authority for Bazel, Buf, Rust, and Go,
and mise installs all of them. Bazel never downloads or installs these tools
itself. The local `projects/mise-bazel` module links
each tool pinned there from mise's installs into its own Bazel repository, fetched
only when a target uses it, and provides Go and Rust as native toolchains. Their compiler, standard-library, and
executable files become hashed build inputs, so cached results follow the
installed versions rather than an arbitrary `buf` or `go` on `PATH`. Missing tools
fail with an installation hint. The Rust toolchain supports macOS ARM64 and Linux
x86-64 execution hosts.

Only rule sets remain Bazel dependencies. Buf compiles the scheduler schema into
its descriptor set and runs the WIT plugin, so Bazel builds no protobuf compiler.
On macOS, `.bazelrc` points C toolchain detection at `/usr/bin/clang` so a mise
shim earlier on `PATH` cannot stand in for the system compiler.

The cached build uses the `.config/mise/config.toml` pins, independent of local
tool-version overrides. Keep the generated tool lock aligned after editing those pins:

```sh
mise run --skip-tools bazel:lock
mise run --skip-tools bazel:setup
```

Commit `.config/mise/mise.lock` with pin updates. It covers every configured repository tool.
The first Go version remains the default for local development and native Bazel
builds. The lockfile uses format 3 so older pinned mise releases can still read it.

The build task runs `bazel build //...`; the test task runs `bazel test //...`.
Arguments are forwarded to Bazel, so a test can be selected without changing
the task:

```sh
mise run --skip-tools bazel:test -- --test_filter=TestGenerate
```

Useful direct target labels include:

| Target | Result |
| --- | --- |
| `//rsworkspace/crates/platform/trogon-config-value:trogon-config-value` | Native Rust library |
| `//rsworkspace/crates/platform/trogon-config-value:trogon-config-value_test` | Rust unit tests for one crate |
| `//goworkspace/cmd/protoc-gen-wit:protoc-gen-wit` | Native Go generator binary |
| `//goworkspace/internal/protowit:protowit_test` | Existing generator tests with pinned Buf |
| `//goworkspace/internal/spirepolicy:spirepolicy_test` | Existing CEL policy tests |
| `//proto/trogonai/scheduler/schedules/v1:schedule_status_descriptor` | Protobuf descriptor set |
| `//proto/trogonai/scheduler/schedules/v1:schedule_status_wit` | Generated WIT artifact |
| `//.github:actionlint_test` | actionlint over every GitHub workflow |

The generator test launcher stages the original scheduler fixture sources and
snapshots as runfiles. Its scoped Buf module contains only those dependency-free
schemas, avoiding unrelated registry dependencies. The selected descriptors are
the same as those produced with the repository's full Buf module. The Go test
sources and assertions are unchanged.

## Add a Rust crate

Every crate's `BUILD.bazel` loads `workspace_crate` from
`.config/bazel/rust.bzl` and calls it, with no arguments for most crates. The `cargo_workspace` module
extension runs `cargo metadata` against `rsworkspace/Cargo.toml` and reruns
when a member manifest, the lockfile, or a target directory changes, so the
library, binaries, integration tests, examples, features, and workspace
dependencies all come from Cargo. External dependencies come from crate_universe
through the same `Cargo.toml`. Bazel builds each crate with all of its features,
as `cargo test --all-features` does.

What Cargo cannot express goes in the crate's `BUILD.bazel` as arguments to
`workspace_crate`, so `Cargo.toml` stays Cargo-only: compile and run time data,
test tags such as `requires-network` and `manual`, skipped unit tests, and extra
build environment. For example:

```starlark
workspace_crate(
    integration_tests = {
        # Starts a NATS server in a container through Docker.
        "echo_conformance": {"tags": ["requires-network"]},
    },
)
```

The arguments are documented in the `workspace_crate` docstring in
`.config/bazel/rust.bzl`, and each one carries a comment saying why it is
needed. Targets that are not crate settings, such as a `filegroup` another
package reads, are ordinary rules in the same `BUILD.bazel`. The build and test tasks find new packages through `//...`,
and `CI Bazel` runs for any change under `rsworkspace/crates`, so no list needs
updating.

Tests that start NATS through Docker run as part of `bazel test //...`, as they
do under Cargo. Tests that scan sources outside their package, load wasm
components built by `cargo component`, or shell out to Cargo (`trybuild`, the
WIT feature build) are tagged `manual` and run in the `CI Cargo` workflow.

The dylint library in `rsworkspace/dylints/trogonai_dylint` has no Bazel
target. It is a separate Cargo workspace with its own lockfile that links
`rustc_private` crates from a pinned nightly toolchain with `rustc-dev`, which
the Bazel Rust toolchain does not provide; the `CI Dylint` workflow builds and
tests it.

## Inspect dependencies

Print the dependency graph of every Bazel target in Graphviz DOT format:

```sh
mise run --skip-tools bazel:graph
```

Inspect the commands, tools, and files involved in WIT generation:

```sh
mise exec bazel -- bazel aquery 'deps(//proto/trogonai/scheduler/schedules/v1:schedule_status_wit)'
```

The schema and compiled Go plugin are declared inputs to generation. Changes to
either can invalidate that action. The `ActionKey` displayed by `aquery` alone is
not a complete file-content-sensitive cache identity; inspect the input graph
and actual cache results as well.

## GitHub Actions caching

The `CI Bazel` workflow restores the repository download cache and action disk
cache from GitHub Actions. Successful pushes to `main` publish cache entries.
Pull requests can restore available entries and run all checks, but do not save
them.

Cache archives have an immutable key for each commit and workflow
attempt, with a restore prefix partitioned by runner operating system,
architecture, and the hashes of `.config/mise/config.toml` and
`.config/mise/mise.lock`. Changes
to managed tool configuration start a new cache namespace. Source-only updates
can add new action results to a later archive. Bazel's own content keys decide
which entries in a restored archive remain reusable.

This cache follows GitHub's repository and branch access rules and eviction
limits. It accelerates Actions jobs; it is not a globally writable cache endpoint
for developer machines or agents. A future self-hosted remote cache can use the
same native targets without replacing the build graph.

## Configure a separate remote cache

Keep local cache configuration in the ignored `.bazelrc.local` file. For example,
point Bazel at an authenticated endpoint using its documented remote-cache
configuration, with credentials supplied outside version control. Avoid adding
credentials to build definitions or action arguments.

Remote cache clients must use compatible toolchains, flags, and execution
platforms. macOS compilation outputs are not interchangeable with Linux outputs.
See [Bazel remote caching](https://bazel.build/remote/caching) for supported
protocols, authentication, and read/write controls.

## Extend the migration

Add native targets next to the code they build, including all consumed files,
tools, and generated artifacts. A new check belongs in Bazel; add a workflow of
its own only when it cannot run as a Bazel target, and list it in
[What runs outside Bazel](#what-runs-outside-bazel). Release automation and the
Dagger pipelines are not Bazel targets.

The root `MODULE.bazel` only declares the module, because Bazel requires it at
the repository root. The root `BUILD.bazel` is empty and exists because
crate_universe resolves `//:MODULE.bazel`, which needs a root package. It includes `.config/bazel/deps.MODULE.bazel`, which pins
rule sets, toolchains, and dependency extensions. Rust dependencies come from the Cargo workspace:
crate_universe reads `rsworkspace/Cargo.toml` and `rsworkspace/Cargo.lock`, so
Bazel and Cargo resolve the same versions with no second list to keep aligned.
A crate takes its external dependencies from its own `Cargo.toml` through
`all_crate_deps` in `@crates//:defs.bzl`.
Commit `MODULE.bazel.lock` updates generated by Bazel along with dependency
changes. Platform-dependent extension metadata may be refreshed on another
supported host.

Do not share one mutable Bazel output directory between concurrent checkouts.
Use Bazel's disk or remote action cache for result sharing.
