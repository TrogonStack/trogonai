# internal_route.x_internal_strip.v1

Reference Tier-3 redaction skill stub. WASM authoring is operator work; this directory ships the manifest and behavior contract only.

## Intent

Remove keys matching `x-internal-*` (case-insensitive) from nested JSON objects attached to message metadata and extension bags. Values are dropped entirely so internal routing hints never cross the gateway egress boundary.

## ABI

```text
redact_part(in_ptr, in_len) -> (out_ptr, out_len)
```

Guest receives a JSON object fragment; returns the same object shape without internal keys.

## Paths

See `internal_route.x_internal_strip.v1.skill.toml` for `applies_to_paths` and method scope.
