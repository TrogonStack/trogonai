---
number: "0065"
slug: generation-settings-usage-and-citations-provider-superset
status: draft
date: 2026-09-28
---

# ADR#0065: Generation Settings, Usage, and Citations Grow to the Provider-Neutral Superset of Anthropic, OpenAI, and Gemini

## Context

`ModelSettings`, `TokenUsage`, `AssistantMessageFailureReason`, and
`ContentBlock` each record a fact about one generation, and each was modeled
against whichever provider motivated it first. A survey across Anthropic,
OpenAI, and Gemini's own request and response shapes found places where
the three agree closely enough that the agreement itself is the fact worth
recording, not any one provider's name for it: tool-forcing, structured
output, a handful of sampling scalars, reasoning-token accounting, per-modality
token accounting, a pre-generation refusal outcome, and text grounding. Where
they disagree, this ADR follows [ADR#0062](./0062-runtime-owned-settings-and-platform-declarations.md)'s
rule that a control only one provider exposes travels as a fact under that
control's own name, or through `ModelSettings.raw_settings`, never generalized
into the shared schema on the strength of one vendor.

None of this is safety policy, replay behavior, or new event lifecycle. It is
a set of narrow, additive extensions to the same `AssistantMessage*`/`CanonicalMessage`
facts this package already records, under [ADR#0064](./0064-schema-constraints-as-documentation.md)'s
rule that a `buf.validate` annotation in this package restates an invariant
already stated in prose, and enforces nothing on its own.

## Decision

### `ModelSettings` gains `ToolChoice`, `ResponseFormat`, and more sampling scalars

`ToolChoice` (`tool_choice = 7`) records whether a generation was left free to
call a tool, forbidden from calling one, forced to call some tool, or forced to
call one of a named set. All three providers expose these modes under their
own names. `ResponseFormat` (`response_format = 8`) records
whether a generation was steered toward unconstrained text, schema-free JSON,
or JSON conforming to a schema; OpenAI and Gemini expose this three-way choice
directly, and Anthropic achieves the same outcome by forcing a tool call,
which is already a `ToolChoice` fact rather than a distinct `ResponseFormat`
one for that provider. The schema itself travels as opaque JSON text
(`schema_json`), the same pattern `ToolUseBlock.input_json` already uses for a
request-sized document that no reader in this package interprets; it is not
claim-checked through an `ArtifactRef`, because it is bounded schema input, not
the unbounded payload `raw_settings` exists to hold out of line.

`top_k`, `seed`, `presence_penalty`, and `frequency_penalty` (`9`-`12`) join
`temperature` and `top_p` as optional scalars with the same explicit-presence
semantics: unset means the provider's own default applied. Each is one that at
least two of the three providers expose under a matching definition;
`candidate_count` and `logprobs` were considered and dropped, because this
package already commits one `CanonicalMessage` per generation, and both
controls only mean something when a generation can return more than one.

`parallel_tool_calls_allowed` (`13`) records whether more than one tool call
was permitted in a single turn. OpenAI exposes this directly as
`parallel_tool_calls`; Anthropic exposes the same control inverted, as
`disable_parallel_tool_use`. The field takes OpenAI's polarity, so `true`
always reads as permission rather than as a double negative, and an Anthropic
adapter negates its own flag when populating it.

### `TokenUsage` gains `reasoning_tokens` and per-modality breakdowns

`reasoning_tokens` (`7`) records the portion of `output_tokens` spent on
reasoning rather than on the emitted reply, for providers that meter it as
billable output. It is defined as *included in* `output_tokens`, not
additional to it, so that every existing sum over this message still holds
for a reader that never looks at the new field; a provider that reports
reasoning tokens as a separate count from its output total is normalized at
the adapter boundary so this invariant holds regardless of how the provider
itself accounts for it.

`input_modality_tokens` and `output_modality_tokens` (`8`, `9`) are each a
`repeated ModalityTokenCount`, decomposing the corresponding total by the
modality (text, image, audio, video, document) each token was spent on. Each
is a decomposition of its total, not an addition to it: summing the entries
reproduces the total they break down, and a reader that wants only the total
keeps reading `input_tokens`/`output_tokens` and ignores the breakdown
entirely.

### `AssistantMessageFailureReason` gains a pre-generation refusal reason

`ASSISTANT_MESSAGE_FAILURE_REASON_INPUT_REJECTED` (`5`) records that the
provider refused the request before producing any output, for example a
prompt blocked by policy pre-generation. This is distinct from
`FinishReason.CONTENT_FILTER` and `FinishReason.REFUSAL`, which both describe
a generation that started and then stopped partway through; a rejection that
never started has no partial `CanonicalMessage` to complete, which is exactly
why it belongs on `AssistantMessageFailed` rather than as a new
`FinishReason` value. The existing `CONTENT_FILTER` and `REFUSAL`
reasons in `assistant_message_completed.proto` gain a one-line cross-reference to this arm so the distinction is
visible from either enum.

### `ContentBlock` gains a `citations` field scoped to `text`

`Citation` (new `citation.proto`) attributes a span of a `text` content block
to the source that grounds it: a `TextSpan` (start/end in Unicode code
points, not UTF-8 bytes, matching how Anthropic and OpenAI themselves report
citation offsets against decoded string positions rather than encoded byte
positions), an optional `cited_text` excerpt, and a `source` naming exactly
one of an already-recorded artifact (`CitedArtifact`, joined by
`ArtifactRef` the same way every other artifact reference in this package
is, plus an optional typed location inside it, either a `PageRange` or a
character `TextSpan`), a web URL with an optional title (`CitedUrl`), or a
tool result already recorded in the session (`CitedToolResult`, joined by
`tool_use_id` the same way `ToolResultBlock` joins to its `ToolUseBlock`).
No confidence score is carried, for lack of cross-provider evidence that one
exists in a comparable form.

`citations` is added as `ContentBlock.citations = 8`, a sibling repeated
field, rather than as new structure on the existing `text` oneof member.
`text` is a bare `string` on the wire today; giving it internal structure
would change the field's type, which `buf breaking` under `WIRE_JSON`
rejects and every existing reader would break on. A new sibling field is
additive by construction. A
message-level CEL rule on `ContentBlock` restates the constraint that
`citations` is only ever populated when `kind` is `text`.

### What was surveyed and not adopted

Safety-category taxonomies, an `update_mask`-style partial-write contract, a
general-purpose JSON Schema message, cached-content resources, and Live-API
streaming shapes were all considered and dropped: each is either a policy
taxonomy this package deliberately keeps out of the wire schema, or a shape
whose problem this package already solves differently (opaque JSON text for
schemas, claim-checked artifacts for large payloads). Async tool scheduling
is a real gap, but a large enough decision to warrant its own ADR rather than
riding in on this one. Context compaction already exists in this package
(`compacted.proto`) and is untouched here.

## Alternatives Considered

### Restructure `ContentBlock.text` into a message with an embedded span list

Rejected in favor of the sibling-field design above: it is a breaking wire
change to the shape every existing reader of `text` already depends on, for
no benefit the sibling field does not already provide.

### Record citation spans in UTF-8 byte offsets, matching `ByteRange`

Rejected because `ByteRange` is defined over opaque resource bytes, a
different unit and a different subject than a decoded text string; reusing
it here would risk a citation offset landing inside a multi-byte character,
which the code-point-based `TextSpan` cannot do, and which neither Anthropic
nor OpenAI's own citation offsets do either.

### Add a confidence score to `Citation`

Rejected for lack of a provider that reports one in a form the other two
providers agree with; adding it now would be inventing a field ahead of the
evidence this package requires for every other addition here.

### Give `parallel_tool_calls_allowed` Anthropic's polarity instead of OpenAI's

Rejected because a field named for what it forbids reads as a double
negative once negated again by a caller who wants to ask "is this allowed";
OpenAI's naming is the one that reads correctly as a plain permission fact,
and Anthropic's own flag is a one-line negation away from it at the adapter.

## Consequences

- `model_settings.proto`, `token_usage.proto`, `assistant_message_failed.proto`,
  `assistant_message_completed.proto`, and `message.proto` each gain new
  fields, messages, and enum values, all additive under `WIRE_JSON`. A new
  `citation.proto` file is added and imported from `message.proto`.
- Every new field carries the same explicit unset-means-provider-default
  semantics the package already uses; no new field changes the meaning of an
  existing one.
- An adapter for any given provider populates only the fields that provider
  actually reports; the message shape does not require faking a value for a
  control a provider does not have.
- The shapes surveyed and not adopted are recorded here as considered and
  rejected, so they are not re-proposed without new evidence.

## References

- `proto/trogonai/session/sessions/v1alpha1/model_settings.proto`
- `proto/trogonai/session/sessions/v1alpha1/token_usage.proto`
- `proto/trogonai/session/sessions/v1alpha1/assistant_message_failed.proto`
- `proto/trogonai/session/sessions/v1alpha1/assistant_message_completed.proto`
- `proto/trogonai/session/sessions/v1alpha1/message.proto`
- `proto/trogonai/session/sessions/v1alpha1/citation.proto`
- [ADR#0062](./0062-runtime-owned-settings-and-platform-declarations.md), for
  the rule that a provider-specific control travels under its own name rather
  than generalized into the shared schema.
- [ADR#0064](./0064-schema-constraints-as-documentation.md), for the
  documentation-only status of every `buf.validate` annotation added here.
