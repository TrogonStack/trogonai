# Declaration readers

These SDK maintenance modules read compiler declarations for compatibility work.
They do not enforce CI, provide a mapping, or establish runtime parity.

Run the Python foundation tests from this directory:

```sh
mise exec -- python -m unittest discover -s tests -v
```

Rust input must be freshly generated rustdoc JSON format 57 using
`nightly-2026-04-16`. The format number does not attest which compiler produced
the bytes; callers must select the pinned toolchain. Portable fixtures model
small compiler constructs and are never trusted as current SDK output.

Automation belongs beside the SDK under `.config/mise/`, following shared
[ADR 6647967335](https://github.com/straw-hat-team/adr/blob/main/src/adrs/6647967335/README.md)
and [ADR 9316364013](https://github.com/straw-hat-team/adr/blob/main/src/adrs/9316364013/README.md).
