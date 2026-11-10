# Build and test with Bazel

Bazel is available alongside the existing Cargo, Go, Buf, and pnpm workflows.
Its current targets build the protobuf-to-WIT generator, test the Go workspace,
test the Rust configuration-value crate, and generate WIT from the scheduler's
`ScheduleStatus` schema. Existing CI remains responsible for the rest of the
repository, Rust lint and coverage gates, Go race detection, and committed
generated-code checks.

## Run the native targets

Install the pinned build tools, then invoke the repository tasks:

```sh
mise run --skip-tools bazel:setup
mise run --skip-tools bazel:build
mise run --skip-tools bazel:test
```

`mise.toml` is the version authority for Bazel, Buf, Rust, and Go. Bazel imports
mise's installed Rust and Go SDKs as native toolchains, including their compiler
and standard-library files as build inputs. It resolves the real Rust toolchain
rather than running rustup launchers inside build actions. Missing SDKs fail with
an installation hint; Bazel does not install them during a build.

Buf comes from the generated `.config/bazel/mise.lock` through
[`rules_mise`](https://github.com/amartani/rules_mise). Bazel materializes the
locked binary for its sandbox and hashes it as a test input. It does not rely on
an arbitrary `buf` executable on `PATH`. Rule sets and the protobuf compiler
remain Bazel dependencies. The supported execution hosts are macOS ARM64 and
Linux x86-64.

The cached build uses the root `mise.toml` pins, independent of local tool-version
overrides. Keep the generated tool lock aligned after editing those pins:

```sh
mise run --skip-tools bazel:lock
mise run --skip-tools bazel:setup
```

Commit `.config/bazel/mise.lock` with pin updates. Bazel rejects mismatched pins,
missing Buf platform entries, and missing Buf SHA256 checksums. The scoped lock is
generated from `mise.toml`; it introduces no second editable version list and
does not change locked-install behavior in existing CI workflows.

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
architecture, and the hashes of `mise.toml` and `.config/bazel/mise.lock`. Changes
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
