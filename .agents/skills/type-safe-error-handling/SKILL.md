---
name: type-safe-error-handling
description: Replace string-matched error classification with typed domain errors, value objects, and explicit boundary mappings. Use when code derives behavior from error messages, substrings, magic prefixes, loosely typed status strings, denial reasons, protocol categories, validation messages, or other stringly typed logic that should be represented at the data type level.
allowed-tools:
  - Bash
  - Read
  - Edit
  - Grep
---

# Type-Safe Error Handling

Use this skill when code lets human-readable text carry domain meaning. The
agent should learn the shape of the domain, then make the semantic decision live
in a type instead of in wording.

## Mental Model

A message can describe a fact, but once behavior depends on that message the
message has become an accidental API. If changing a sentence can change an auth
decision, retry policy, denial category, telemetry status, or protocol response,
the code is missing a type.

Think in terms of information flow:

- Where is the domain fact first known?
- Where is it collapsed into a `String`, status label, magic prefix, or loosely
  typed field?
- Which later decision is trying to reconstruct that fact?
- Which existing enum, value object, source error, or wire type should carry the
  fact instead?

## Principles

- Name the semantic fact at the source. `UnknownAccount`, `InvalidCredentials`,
  `MissingCredentialMaterial`, or `VerifierUnavailable` are better decision
  inputs than messages containing those words.
- Preserve meaning across layers. A boundary conversion should be
  variant-to-variant or field-to-field, not format-and-parse.
- Keep display text at presentation edges. Logs, diagnostics, and user-facing
  messages may stay stable, but they should explain typed decisions rather than
  drive them.
- Preserve source chains when they explain cause. Do not choose between correct
  classification and observability if the error model can represent both.
- Let wire compatibility and domain modeling coexist. Keep external strings,
  codes, or protobuf fields stable when required, but convert to richer domain
  types inside the owning package.
- Respect local architecture. In Rust, keep value objects inside the owning
  crate unless [ADR#0002](../../../docs/adr/0002-rust-crate-boundaries.md) justifies a package boundary. For wire and persistence
  contracts, follow [ADR#0009](../../../docs/adr/0009-protocol-buffers-wire-contracts.md).

## Key Examples

Typed boundary mapping:

```rust
impl From<ApiKeyError> for AuthCalloutError {
    fn from(error: ApiKeyError) -> Self {
        match error {
            ApiKeyError::Empty => CredentialError::InvalidRequest(...).into(),
            ApiKeyError::Unknown => CredentialError::InvalidCredentials(...).into(),
            ApiKeyError::AudienceMismatch { .. } => CredentialError::InvalidRequest(...).into(),
            ApiKeyError::CallerIdDerivation(_) => CredentialError::InvalidCredentials(...).into(),
        }
    }
}
```

String-derived behavior:

```rust
let category = if error.to_string().contains("not found") {
    DenialCategory::InvalidCredentials
} else {
    DenialCategory::InternalError
};
```

The first shape carries meaning through variants. The second makes wording part
of control flow.

Local examples to learn from:

- `rsworkspace/crates/a2a-auth-callout/src/denial_category.rs` classifies
  `CredentialVerification(String)` with `msg.contains(...)`. The category
  decision belongs in typed credential or domain errors that map directly to
  `DenialCategory`.
- `rsworkspace/crates/a2a-auth-callout/src/account_resolver.rs` converts
  `AccountResolverError` through `value.to_string()`. The resolver already knows
  whether the problem is `EmptyRequest` or `Unknown`; that distinction should
  survive until category mapping.
- `ApiKeyError`-style mappings should stay variant-to-variant. Empty input,
  unknown key, caller-id derivation failure, and audience mismatch are different
  facts even if their display messages all end up near credential verification.

## Smells

Pause and reason from the principles when you see:

- `contains`, `starts_with`, regexes, or equality checks against error messages.
- `map_err(|e| SomeError(e.to_string()))` before another layer classifies the
  result.
- catch-all `String` variants used for multiple semantic cases.
- tests proving semantic behavior through substrings.
- protocol categories inferred from local wording instead of typed tags.

## Verification

Confidence comes from tests that prove the type-level contract:

- each source variant maps to the intended typed category.
- wire-visible strings or codes remain compatible when compatibility matters.
- wrapped sources remain available through `source()` where supported.
- unknown or internal errors do not accidentally become user-correctable errors.

String assertions are allowed for stable display text, but not as the only proof
of semantic behavior.

## Done

The task is done when a future wording change cannot change behavior, and a
reviewer can point to the type that owns each semantic decision.
