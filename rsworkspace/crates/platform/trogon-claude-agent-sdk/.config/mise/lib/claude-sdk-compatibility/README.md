# Declaration readers

These SDK maintenance modules read compiler declarations for compatibility work.
They do not enforce CI, provide a mapping, or establish runtime parity.

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

Reviewed source retrieval, complete API mappings, stale-reference and contract
source validation, actual Rust contract execution, a CLI, mise tasks, and CI
integration remain separate work. A weekly upstream report is separate from
deterministic enforcement. Declaration shapes alone establish no runtime parity.

Automation belongs beside the SDK under `.config/mise/`, following shared
[ADR 6647967335](https://github.com/straw-hat-team/adr/blob/main/src/adrs/6647967335/README.md)
and [ADR 9316364013](https://github.com/straw-hat-team/adr/blob/main/src/adrs/9316364013/README.md).
