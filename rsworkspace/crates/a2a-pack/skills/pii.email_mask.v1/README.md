# pii.email_mask.v1

Reference Tier-3 redaction skill stub. WASM authoring is operator work; this directory ships the manifest and behavior contract only.

## Intent

Mask email-shaped substrings in A2A message and artifact part text before the payload leaves the trust boundary. Examples:

- `alice@example.com` → `[REDACTED_EMAIL]`
- Preserves surrounding prose and non-email tokens.

## ABI

Each targeted JSON value is serialized and passed to the guest export:

```text
redact_part(in_ptr, in_len) -> (out_ptr, out_len)
```

Input and output are UTF-8 JSON fragments matching the host-selected path (typically a string leaf under `message.parts[*].text` or `content`).

## Paths

See `pii.email_mask.v1.skill.toml` for `applies_to_paths` and method scope.
