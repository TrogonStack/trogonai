# credentials.bearer_redact.v1

Reference Tier-3 redaction skill stub. WASM authoring is operator work; this directory ships the manifest and behavior contract only.

## Intent

Strip `Authorization: Bearer …` substrings from part text and nested string fields selected by the manifest paths. Literal `Bearer` tokens followed by JWT or opaque secrets are replaced with `[REDACTED_BEARER]` while leaving non-bearer headers intact.

## ABI

```text
redact_part(in_ptr, in_len) -> (out_ptr, out_len)
```

Guest receives JSON fragments; returns the redacted JSON fragment with the same shape.

## Paths

See `credentials.bearer_redact.v1.skill.toml` for `applies_to_paths` and method scope.
