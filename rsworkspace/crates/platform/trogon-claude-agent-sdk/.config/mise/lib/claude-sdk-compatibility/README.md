# Declaration readers

These SDK maintenance modules read compiler declarations for compatibility work.
They do not establish runtime parity. `behaviors.json` maps runtime behavior
against both upstream SDKs. Only its tests run in CI: `claude-sdk:test` depends
on `claude-sdk:behavior-map`, which runs `tests/test_behavior_map.py`, and on
`claude-sdk:gate-z`, which runs `tests/test_gate_z.py`.

Run the Python foundation tests from this directory:

```sh
mise exec -- python -m unittest discover -s tests -v
```

Install the locked TypeScript compiler and run its reader tests here:

```sh
mise exec -- pnpm install --frozen-lockfile
mise exec -- node --test tests/test_upstream_typescript.mjs
```

`rust_surface.extract(document)` returns a dictionary keyed by public export,
variant, and member names. Resolved types contain canonical public aliases or
external paths with structured generic arguments, without compiler IDs. Private
named fields are omitted; stripped-field metadata and private tuple positions
remain visible. Derived, synthetic, and blanket trait implementations are
excluded. Unsupported item and type forms fail with `GateError`.

`compiled_tests.compiled_tests(document)` reads a nextest JSON listing. Its
`selectors` are listed, nonignored tests that match the listing's filters;
`ignored` and `filtered` hold the other listed selectors. Its evidence is always
`compiled-and-listed`. No field claims that any test executed or passed.

Python `upstream_python.extract(bytes, expected_hash)` and the exported
TypeScript `extract(bytes, expectedHash)` accept a reviewed SHA256 pin and at
most 2 MiB of source. They parse declarations without executing the SDK. Python
selects `ClaudeAgentOptions` annotations/defaults; TypeScript selects `Options`
property types, optionality, and readonly modifiers. Both select the
`PermissionMode` and `SettingSource` string literal aliases. TypeScript uses the locked compiler 6.0.3.

Rust input must be freshly generated rustdoc JSON format 57 using
`nightly-2026-04-16`. The format number does not attest which compiler produced
the bytes; callers must select the pinned toolchain. Portable fixtures model
small compiler constructs and are never trusted as current SDK output.

The Rust reader rejects associated generic constraints, equality predicates,
function pointer types, trait object types, and other unimplemented type forms.
Public glob and external reexports without local nodes are rejected. Extending
these formats requires deliberate source changes and regression coverage.

Reviewed source retrieval, complete API mappings, actual Rust contract execution, a CLI, mise tasks, and CI
integration remain separate work. A weekly upstream report is separate from
deterministic enforcement. Declaration shapes alone establish no runtime parity.

## Runtime behavior mapping

Declaration shapes say nothing about what the SDK does at runtime. Each
`behaviors.json` row describes one runtime behavior, cites the Python and
TypeScript code that implements it, and records which behavior Rust follows.
The rows let a reviewer see where both SDKs agree, where they differ, and where
Rust deliberately does something else.

`behavior-pins.json` pins the reviewed upstream bytes. Python files are pinned
by repository revision and per-file SHA256. The TypeScript package is pinned by
npm tarball integrity and by the SHA256 of each cited member. Pin URLs are
derived from the pinned fields, so a pin cannot silently point somewhere else.

Evidence fails closed when upstream changes:

- Python evidence names a line range and the SHA256 of those exact bytes.
- TypeScript evidence names an anchor string, its occurrence, the total count
  of that anchor, and the SHA256 of a byte span around it. The published
  bundle is minified, so lines and identifiers are not stable references.
- A missing anchor, a changed anchor count, or a range outside the file is a
  `stale-reference`. A changed span is `shape-changed`. Unreviewed bytes are
  `source-pin-mismatch`.
- A side that has no such behavior says so with `status: "absent"` and a
  reason, never by omission.

Each row carries an upstream verdict and a decision:

- `agree` rows use `upstream`, or `rust-adaptation` with a reason when Rust
  adds a safety bound neither SDK has.
- `disagree` rows must choose `follow-python` or `follow-typescript` with a
  reason. Rust cannot pick a third behavior without first recording which SDK
  it diverges from.

`validate(behaviors, pins, sources)` does not fetch anything. The caller
supplies the reviewed bytes, so validation stays deterministic and never runs
upstream code. Every pinned file must be cited by some row, so stale pins are
rejected too.

`contracts` names the Rust tests that pin each behavior. An integration test is
a nextest `binary::test` selector, which `contracts_check` requires to be in the
compiled selectors, such as those from `compiled_tests`. A unit test is a
crate-relative `src/<path>.rs::<name>` selector, which must be one of the
`#[test]` or `#[tokio::test]` functions that `unit_test_selectors` finds in the
given sources. The mapping test checks both against the crate's top-level
integration tests and every file under `src/`.
`validate(..., require_contracts=True)` rejects any row without one. The
mapping alone does not establish runtime parity.

## Gate Z

`gate_z.py` is the parity bar. It fails while any of these hold:

- A feature in `../claude-sdk-freshness/mapping.json` is not `supported`.
- A behavior row decides anything other than `upstream`, `follow-typescript` or
  `follow-python`, or follows Python where TypeScript has a counterpart. Python
  is followed only for a form TypeScript lacks, which review keeps opt-in.
- A behavior row is a `rust-adaptation` outside the TLS rows m19 to m25 and
  `m26-python-tls-trust-directory-lookup`, or one of those rows gives a reason
  that does not end with the owner's rustls decision under ADR 0015. The one
  other accepted adaptation is `m18-python-system-proxy-undefined-reads`,
  whose upstream behavior is a crash or an uninitialized read. Rust does not
  copy a defect, and that row's reason must end with `DEFECT_REASON`. The
  other is `r33-typescript-settings-file-depth`, whose bound the owner fixed
  instead of matching the depth V8 reaches, and its reason must end with
  `DEPTH_REASON`.
- A behavior row has no contracts.
- `openssl`, `openssl-sys`, `openssl-src` or `native-tls` is reachable from the
  SDK in the workspace `Cargo.lock`.

Each check returns every gap it finds, so one failing run lists them all.

The reviewed-bytes test reads `upstream/typescript/<tarball>` and
`upstream/python/<path>` in this directory and is skipped when they are
absent.

Automation belongs beside the SDK under `.config/mise/`, following shared
[ADR 6647967335](https://github.com/straw-hat-team/adr/blob/main/src/adrs/6647967335/README.md)
and [ADR 9316364013](https://github.com/straw-hat-team/adr/blob/main/src/adrs/9316364013/README.md).
