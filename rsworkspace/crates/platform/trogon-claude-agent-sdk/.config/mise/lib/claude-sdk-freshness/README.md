# Claude SDK upstream compatibility monitor

This maintenance tool compares the official Python SDK source and published
TypeScript SDK declarations with a reviewed baseline. The GitHub workflow runs
weekly, alongside manual dispatch, and reports review work in a deduplicated
issue. It never edits the Rust SDK or advances the baseline automatically.

Use the [Rust lifecycle decision](../../../../docs/explanation/lifecycle.md)
when mapping upstream changes to client ownership, run completion, callbacks
and cleanup.

The layout follows [ADR#6647967335: mise tasks and shared code](https://github.com/straw-hat-team/adr/blob/main/src/adrs/6647967335/README.md).
The SDK crate owns its `mise.toml`, file tasks and maintenance data. Shared
implementation, parser dependencies and reviewed inventories live under the
crate's `.config/mise/lib/claude-sdk-freshness/`. File tasks resolve those helpers
through the crate's `MISE_PROJECT_ROOT` and inherit repository tool versions.
Repository-root GitHub workflow entrypoints explicitly select the SDK directory
before invoking its tasks. The workflow retains its dedicated
`github:workflows:claude-sdk-freshness:test` and
`github:workflows:claude-sdk-freshness:check` tasks.

## What the report means

Python's `ast` module reads source without importing the SDK. The TypeScript
Compiler API reads published declarations without importing the SDK runtime.
The inventory includes exported types, fields, methods, aliases and reachable
local declaration types. Nested field optionality, union members, defaults and
signatures participate in comparison. Source line movement does not constitute
a type change.

The TypeScript Git repository does not contain the SDK core implementation, so
checking that repository alone is insufficient. Release checks download the
published npm package and the Python source distribution, verify registry
digests, and inspect selected source files. They do not install or execute
upstream package code. Runtime source hashes also trigger behavior review when
declarations have not changed. A hash difference is a review signal, not proof
that behavior changed.

`baseline.json` records reviewed release versions, artifact provenance, API
inventories, extractor fingerprints and implementation hashes. `mapping.json`
connects upstream symbols to Rust APIs, named regression tests and the reason
for their status:

| Status | Meaning |
| --- | --- |
| `supported` | The scoped feature has a Rust implementation and linked regression evidence. |
| `partial` | The feature exists with an explicitly stated remaining limitation. |
| `deferred` | The upstream functionality needs further Rust implementation or design. |
| `rust_difference` | Rust deliberately expresses or defaults the feature differently. |
| `out_of_scope` | The feature is outside this native library's declared contract. |

Core mappings follow the reviewed Python-aligned API: `ClaudeAgentOptions`
stores optional `cli_path`, `cwd`, unrestricted `model`, `permission_mode` and
`setting_sources`; `ClaudeSdkClient` uses `connect` and consuming async
`disconnect(self)`. TypeScript counterparts use `pathToClaudeCodeExecutable`,
`permissionMode`, `settingSources` and `Query.close`. Exact member mappings
override the partial aggregate options mapping.

An omitted permission mode differs from explicit `PermissionMode::Default`.
An omitted settings-source list inherits CLI defaults; an explicit empty list
disables user/project/local filesystem settings. The pinned Python and
TypeScript `Options` documentation agree on these defaults. MCP discovery has
its own Rust isolation default and is tracked separately.

An unmapped symbol means its Rust mapping has not been reviewed. It does not
prove missing functionality. New and changed upstream symbols require review
even when an older feature mapping says `supported`. Aggregate mappings are
conservative; an exact member mapping takes precedence over a parent prefix.

The monitor cannot prove functional parity by comparing type names. Rust keeps
validated value objects, enums, ownership and cancellation semantics. Keep
wire behavior in public Rust regressions, review the upstream implementation
changes, and run the native conformance checks appropriate to the change.
Passing static inspection is not a claim of live model-session conformance.
Named-test validation checks references, not test execution. This report-only
monitor is separate from the deterministic compatibility CI gate required
before runtime slices can land.

## Run a check

Run maintenance commands from the SDK crate directory. Its nested mise
configuration keeps these tasks and their data together. From the repository
root, select the crate first:

```sh
cd rsworkspace/crates/platform/trogon-claude-agent-sdk
```

Install the pinned parser dependency without package lifecycle scripts:

```sh
mise run claude-sdk:install
```

Check published releases and write local reports:

```sh
mise run claude-sdk:freshness
```

Run the monitor's offline regression suite:

```sh
mise run claude-sdk:test
```

The test, freshness and baseline tasks depend on `claude-sdk:install`, so local
commands and GitHub Actions share the same pinned dependency setup. Their task
definitions live under `.config/mise/tasks/claude-sdk/`. Arguments after a task
name are forwarded to the checker or test runner.

Local checkouts can be inspected without downloading upstream releases:

```sh
mise run claude-sdk:freshness \
  --python-source /path/to/claude-agent-sdk-python \
  --typescript-package /path/to/extracted/npm/package \
  --out /path/to/claude-sdk-report
```

The TypeScript path is an extracted published package, not the TypeScript Git
checkout. Local mode and release mode use the same inventories and comparator.
Reports contain upstream package-relative source locations and SDK-relative
Rust evidence paths, not local checkout paths.

## Generated upstream types

`generate_types.mjs` turns the pinned upstream declarations into serde types under
`src/upstream/`. It uses the same pinned TypeScript compiler as the extractor and
reads the exact npm package recorded in `baseline.json`; `types.json` names the
package version, source files, output files and root types. The output keeps the
upstream wire shape and validates nothing:

- every object type keeps keys upstream does not declare in a flattened `extra` map and
  records the key order it was decoded with (`key_order`), so decoding then encoding
  is lossless and byte-identical, including key order;
- typed maps are insertion ordered (`indexmap::IndexMap`) and untyped maps are
  `serde_json::Map`, which keeps insertion order through the crate's `preserve_order`
  feature;
- a string-literal union becomes an enum whose `Unknown(String)` variant
  round-trips unlisted values;
- `field?: T` (omitted), `field: T | null` (null) and `field?: T | null` stay distinct;
- a union of other shapes picks the member that serializes back to the same JSON with
  the fewest undeclared keys (earliest declared on ties, nested unions included),
  otherwise its own `Unknown` variant holds the raw JSON value. A string-literal property
  holding a value the declarations do not list counts as a mismatch, so a discriminant
  picks its own member;
- numbers use `serde_json::Number`, so integers and floats round-trip exactly.

The `Settings` target also lists `fixtures` in `types.json`. `synthesize_fixtures.mjs` writes
upstream-shaped samples from the same declarations (every field set, every union member
visited, one undeclared key in each open object) split into per-family groups, and
`claude-sdk:types-check` fails when the checked-in fixture file drifts.

A target may also list `extern` types (`{"Name": "rust::path"}`) that live in another
generated module instead of being generated again.

```sh
mise run claude-sdk:types        # regenerate from the pinned upstream package
mise run claude-sdk:types-check  # fail when checked-in output differs from it
```

The check downloads the pinned package, verifies its registry integrity against the
baseline and compares the generated bytes. The workflow runs it on pull requests. The
offline regression suite (`claude-sdk:test`) covers the generator itself. Bumping the
baseline version requires updating `types.json` and regenerating in the same change.
Generated files are marked `linguist-generated` and excluded from `rustfmt`.

## Scheduled issue reporting

[The workflow](../../../../../../../../.github/workflows/claude-sdk-freshness.yml) follows the
ACP freshness pattern. It runs Monday at 09:00 UTC and supports manual dispatch.
The report distinguishes release/API drift, implementation review triggers,
declared gaps and unreviewed mappings. Repeated runs update an existing issue
created by `github-actions[bot]` instead of creating a new issue each week.
Issues from other authors cannot redirect or block that update. Full
machine-readable details are retained as workflow artifacts when the issue summary is truncated.

Fetch, integrity or extraction failures fail the check. They are never reported
as a clean comparison or as an upstream feature release. Baseline generation
does not file an issue. Pull request validation has no issue-write permission.
Changing an extractor or its pinned parser requires reviewing a new inventory
against the same upstream releases first. A fingerprint mismatch fails the
comparison rather than attributing parser changes to upstream.

For a monthly cadence, change the schedule to `0 9 1 * *`. The workflow becomes
active after it is merged onto the repository's default branch. Local checks
use `CLAUDE_SDK_FRESHNESS_REPORT_MODE=local` by default. The workflow selects
`issue` to synchronize its report using the workflow token. Unknown or empty
modes fail before the checker runs.

## Review and accept upstream changes

1. Read the API diff and source references, including implementation hash changes.
1. Decide the Rust behavior for each change. Update exact symbol mappings and
   record remaining gaps or intentional differences. Do not copy upstream type
   names simply to make the inventories look alike.
1. Add meaningful regressions through the public Rust API. Check newly required
   fields, unknown values, malformed known messages, cancellation and run ending
   where relevant.
1. Run the Rust checks and any required bounded native conformance tests.
1. Generate a candidate baseline for the exact reviewed releases, then review
   its diff alongside the mappings and Rust changes:

   ```sh
   mise run claude-sdk:baseline \
     --python-version 0.2.179 --typescript-version 0.3.319 \
     --out /path/to/claude-sdk-baseline-review
   ```

The baseline task is a manual maintenance entrypoint; the scheduled workflow
invokes `github:workflows:claude-sdk-freshness:check`, which delegates to
`claude-sdk:freshness` in the SDK crate. The baseline task writes the candidate
review snapshot to `.config/mise/lib/claude-sdk-freshness/baseline.json`, while
`--out` selects its report directory. The checker's baseline-write and
issue-publication mutual exclusion still applies.

Accepting an inventory acknowledges review of upstream changes. It does not
resolve declared partial/deferred features or turn unreviewed mappings into
supported functionality. The issue remains actionable until those distinctions
are explicitly addressed.
