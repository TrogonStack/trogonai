# Build and test with Bazel

Bazel is available alongside the existing Cargo, Go, Buf, and pnpm workflows.
Its current targets build the protobuf-to-WIT generator, test the Go workspace,
test the Rust configuration-value crate, and generate WIT from the scheduler's
`ScheduleStatus` schema. Existing CI remains responsible for the rest of the
repository, Rust lint and coverage gates, Go race detection, and committed
generated-code checks.

## Run the native targets

Install the pinned launcher, then invoke the repository tasks:

```sh
mise install bazelisk
mise run --skip-tools bazel:build
mise run --skip-tools bazel:test
```

Bazelisk selects the Bazel release in `.bazelversion`. Bazel downloads its pinned
Rust, Go, protobuf, and Buf tools rather than invoking Cargo or Go build wrappers.
The initial execution platforms are macOS ARM64 and Linux x86-64.

The build task builds `//:build`; the test task runs `bazel test //...`.
Arguments are forwarded to Bazel, so a test can be selected without changing
the task:

```sh
mise run --skip-tools bazel:test -- --test_filter=TestGenerate
```

Useful direct target labels include:

| Target | Result |
| --- | --- |
| `//rsworkspace/crates/platform/trogon-config-value:trogon-config-value` | Native Rust library |
| `//rsworkspace/crates/platform/trogon-config-value:trogon-config-value_test` | Existing Rust unit tests |
| `//goworkspace/cmd/protoc-gen-wit:protoc-gen-wit` | Native Go generator binary |
| `//goworkspace/internal/protowit:protowit_test` | Existing generator tests with pinned Buf |
| `//goworkspace/internal/spirepolicy:spirepolicy_test` | Existing CEL policy tests |
| `//proto/trogonai/scheduler/schedules/v1:schedule_status_proto` | Protobuf descriptor |
| `//proto/trogonai/scheduler/schedules/v1:schedule_status_wit` | Generated WIT artifact |

The generator test launcher stages the original scheduler fixture sources and
snapshots as runfiles. Its scoped Buf module contains only those dependency-free
schemas, avoiding unrelated registry dependencies. The selected descriptors are
the same as those produced with the repository's full Buf module. The Go test
sources and assertions are unchanged.

## Inspect dependencies

Print the build target's dependency graph in Graphviz DOT format:

```sh
mise run --skip-tools bazel:graph
```

Inspect the commands, tools, and files involved in WIT generation:

```sh
mise exec bazelisk@1.30.0 -- bazelisk aquery 'deps(//proto/trogonai/scheduler/schedules/v1:schedule_status_wit)'
```

The schema and compiled Go plugin are declared inputs to generation. Changes to
either can invalidate that action. The `ActionKey` displayed by `aquery` alone is
not a complete file-content-sensitive cache identity; inspect the input graph
and actual cache results as well.

## GitHub Actions caching

The `CI Bazel` workflow restores Bazelisk downloads, the repository download
cache, and the action disk cache from GitHub Actions. Successful pushes to `main`
publish cache entries. Pull requests can restore available entries and run all
checks, but do not save them.

Cache archives have an immutable key for each commit and workflow
attempt, with a restore prefix partitioned by runner operating system,
architecture, and pinned Bazel version. Source-only updates can therefore add
new action results to a later archive. Bazel's own content keys decide which
entries in a restored archive remain reusable.

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
tools, and generated artifacts. Preserve the existing language checks until
their Bazel equivalents have demonstrated parity. The current targets do not
migrate the remaining Rust crates, TypeScript packages, full Buf module, release
automation, or Dagger pipelines.

`MODULE.bazel` pins rule sets and direct Rust dependencies. The dedicated
`.config/bazel/Cargo.lock` locks the Rust dependency subset; it does not replace
or rewrite `rsworkspace/Cargo.lock`. Keep its dependency versions aligned with
the corresponding Cargo workspace dependencies when updating the migrated crate.
Commit `MODULE.bazel.lock` updates generated by Bazel along with dependency
changes. Platform-dependent extension metadata may be refreshed on another
supported host.

Do not share one mutable Bazel output directory between concurrent checkouts.
Use Bazel's disk or remote action cache for result sharing.
