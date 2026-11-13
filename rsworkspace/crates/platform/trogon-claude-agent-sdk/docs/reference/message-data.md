# Message data reference

These Rust declarations are the messages a session yields. The
[Decoding](#decoding) section describes how CLI frames become them; process
ownership and permission policy are separate runtime concerns.

Comparison sources are pinned [Python types](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py)
and the integrity-verified [TypeScript 0.3.319 declarations](https://registry.npmjs.org/@anthropic-ai/claude-agent-sdk/-/claude-agent-sdk-0.3.319.tgz).
The table entries describe stored shapes and deliberate Rust representations;
they do not assert runtime parity or complete upstream union coverage.

## JSON objects

JsonObject is serde_json::Map<String, serde_json::Value>, used for object-valued
payloads and explicit extension maps. Python uses dict[str, Any] for fields such
as ToolUseBlock.input. TypeScript object and Record types have comparable
object-valued roles, while any and unknown can also represent non-object data.
The Rust alias is not a claim that either SDK exports a type named JsonObject.
The generic serde_json object decoder rejects scalar and array roots while
retaining nested nulls, arrays and objects. This is not an SDK frame decoder.

## Message origin

| Rust declaration | Python source | TypeScript source | Stored representation |
| --- | --- | --- | --- |
| MessageOrigin.kind | MessageOrigin.kind: Required[MessageOriginKind] | SDKMessageOrigin discriminated union | String preserves known and future spellings, including empty and control-character values. |
| MessageOrigin.additional_fields | MessageOrigin is a total=False TypedDict; its docs permit additional undocumented keys | SDKMessageOrigin variant-specific fields | Explicit JsonObject retains fields instead of reproducing the closed TypeScript union. |

MessageOrigin serde keeps its kind discriminator separate from extension keys.
The constructor rejects an extension named kind so it cannot override the
validated discriminator. Known kinds have richer upstream meaning; a future
kind is representable without being treated as a human-origin authorization.

## Message content

| Rust declaration | Python source | TypeScript source | Stored representation |
| --- | --- | --- | --- |
| Content::Text / Content::Blocks | UserMessage.content: str or list[ContentBlock] | SDKUserMessage.message: MessageParam | String or Vec<ContentBlock>; no envelope decoding. |
| Text / Thinking | TextBlock / ThinkingBlock | SDKAssistantMessage.message: BetaMessage | String text, or thinking and signature strings. |
| ToolUse | ToolUseBlock.id/name/input | BetaMessage content tool-use blocks | ToolUseId/ToolName wrappers and JsonObject input. |
| ToolResult | ToolResultBlock.tool_use_id/content/is_error | MessageParam content tool-result blocks | ToolUseId, raw JSON content, and separate NotReported/Success/Error status. |
| ServerToolUse / ServerToolResult | ServerToolUseBlock / ServerToolResultBlock (from `server_tool_use` and `advisor_tool_result`) | BetaMessage server tool blocks | ToolUseId, name, JsonObject input; raw JSON result content. |
| Unknown | Any block kind Python does not parse | TypeScript content references contain a broader block family | The block `type` and the block without it. TypeScript decoding keeps every block it cannot type here; Python decoding skips them as `parse_message` does. |

ToolResultStatus distinguishes absent error status from explicit success and
error. It is a Rust enum representation, not an upstream enum spelling. Raw
JSON can preserve content shapes beyond Python's annotated str/list/None field.

## System messages

| Rust field | Python SystemMessage | TypeScript SDKSystemMessage | Stored representation |
| --- | --- | --- | --- |
| subtype | subtype: str | subtype: 'init' for the named init type | String keeps system subtypes open. |
| data | data: dict[str, Any] | Init metadata fields are declared on the envelope | Explicit JsonObject holds metadata without claiming the TypeScript flat type is identical. |
| session_id | Not a declared Python SystemMessage field | session_id: string | Optional SessionId retains a possible envelope identity. |
| event | TaskStartedMessage, TaskProgressMessage, TaskNotificationMessage, TaskUpdatedMessage, MirrorErrorMessage, HookEventMessage subclasses | SDKTaskStartedMessage and the other system members | SystemEvent holds the Python subclass fields when every annotated value has its annotated type, otherwise Other. |
| raw | Not a dataclass field | The whole message | The message as written; to_sdk_message types it as SDKMessage. |

## Turn results

| Rust field family | Python ResultMessage | TypeScript result source | Stored representation |
| --- | --- | --- | --- |
| subtype / status | subtype: str; is_error: bool | SDKResultSuccess / SDKResultError subtype and is_error | Open String subtype and ResultStatus::Success/Error; no boolean decoder here. |
| duration_ms / duration_api_ms / num_turns | int | number | u64 for nonnegative integral Rust data; upstream annotations alone do not impose this Rust range. |
| session_id / uuid | str / optional str | string / UUID | Required SessionId and optional MessageId. |
| result / total_cost_usd | optional str / optional float | Success result:string and cost:number | Option<String> and Option<f64> also represent absent legacy data. |
| usage / model_usage | optional object; model_usage maps model names | usage / modelUsage | Optional raw JsonObject; TypeScript's camelCase modelUsage is a distinct wire spelling. |
| structured_output | Any | structured_output?: unknown | Option<Value> permits manually represented JSON null separately from absent data. |
| errors / permission_denials / deferred_tool_use | optional list[str] / list[Any] / DeferredToolUse | SDKResultError.errors / permission_denials / deferred_tool_use | Optional string list, raw JSON list and JsonObject; unreported errors remain distinct from a reported empty list. `typed_permission_denials` wraps each raw entry in a `PermissionDenial` with `tool_name`, `tool_use_id`, `tool_input`, `typed_input` and `to_sdk_permission_denial` views. |
| api_error_status / stop_reason / terminal_reason | optional int / str / str | api_error_status?: number or null; stop_reason; TerminalReason | Option<u16> and optional String fields; no status or terminal-reason interpretation is added. |
| origin | optional MessageOrigin | origin?: SDKMessageOrigin | Optional MessageOrigin. |
| result_index / queued_turn_count | Not declared on the pinned Python ResultMessage | result_index?: number / queued_turn_count?: number | Optional u64 metadata. |
| additional_fields | Not a separate Python dataclass field | Declarations include additional version-specific fields | Explicit extension JsonObject retains fields beyond the declared shape. |

A result is the declared outcome of a turn. Its presence here does not implement
run completion, queued-turn accounting, cancellation or process shutdown.

## Assistant messages

| Rust field family | Python AssistantMessage | TypeScript SDKAssistantMessage | Stored representation |
| --- | --- | --- | --- |
| content / model / message_id | list[ContentBlock] / str / optional str | message: BetaMessage nests content/model/id | Flattened Rust fields retain Vec<ContentBlock>, unrestricted String and optional MessageId. |
| parent_tool_use_id / session_id / uuid | optional strings | nullable parent ID, required session_id and UUID | Optional identifier wrappers also represent missing legacy metadata. |
| error / usage / stop_reason | optional error literal / object / string | error enum and nested BetaMessage usage/stop_reason | Optional String/object/String preserve unknown error strings without a new restriction. |
| additional_fields / message_fields | No separate typed extension fields | Envelope and nested message have different scopes | Separate JsonObject values retain manually supplied extension data. |

## User messages

| Rust field family | Python UserMessage | TypeScript SDKUserMessage | Stored representation |
| --- | --- | --- | --- |
| content | str or list[ContentBlock] | message: MessageParam | Content text or block data, flattened from the envelope. |
| parent_tool_use_id / session_id / uuid | optional parent ID and uuid; no separate session_id field | nullable parent ID and optional session_id/UUID | Optional identifier wrappers. |
| origin | optional MessageOrigin | origin?: SDKMessageOrigin | Optional origin object; `MessageOrigin::validated` opts in to refusing a `kind` among the extension fields. |
| tool_use_result | optional dict[str, Any] | tool_use_result?: unknown | Option<Value> retains every JSON kind; None is omitted data and Some(Value::Null) is explicitly present null. Both pinned SDKs pass the wire value through unchanged; tool_use_result_object gives the dictionary view Python annotates, and `typed_tool_use_result(tool_name)` the `sdk-tools` output type TypeScript declares for that tool. |
| additional_fields / message_fields | No separate typed extension fields | Envelope and MessageParam scopes differ | Explicit maps preserve manually supplied outer and nested extension data. |

## Partial events

| Rust field | Python StreamEvent | TypeScript SDKPartialAssistantMessage | Stored representation |
| --- | --- | --- | --- |
| uuid / session_id | str / str | UUID / string | Existing String uuid and infallible SessionId wrapper. |
| event | dict[str, Any] | event: BetaRawMessageStreamEvent | Raw JsonObject rather than a recreated event union. |
| parent_tool_use_id | optional str | string or null | Optional ToolUseId. |
| additional_fields | No separate dataclass field | ttft_ms, user_message_uuid, user_message_uuids, resume_reason and future envelope fields | Explicit JsonObject retains outer metadata separately from event through wire decoding. |
| raw / to_sdk_message | Not a dataclass field | SDKPartialAssistantMessage | The message as written, and its typed TypeScript form with every envelope field typed. |

This declaration does not enable partial messages, decode deltas or reconstruct
assistant content. Those are separate operational review boundaries.

## Message envelope

Message has Assistant, User, System, Result, StreamEvent, RateLimit,
ConversationReset and Unknown variants. RateLimit and ConversationReset are
Python RateLimitEvent and ConversationResetMessage. Unknown holds the `type`,
the message without it and the message as written.

Every message keeps the object the CLI wrote in `raw`, so no field is dropped.
`Message::to_sdk_message` types it as the generated TypeScript `SDKMessage`
from `upstream::messages`, which covers every member the pinned sdk.d.ts
declares; encoding that value gives back `raw`. The per-struct
`to_sdk_message` methods return the matching member, or None when the message
does not match its shape exactly. TypeScript `query()` also yields the
`active_goal` message, which sdk.d.ts declares outside the `SDKMessage` union;
`Message::to_active_goal_message` types it as `SDKActiveGoalMessage`.

## Decoding

`ClaudeAgentOptions::message_decoding` selects the upstream SDK whose decoding
is reproduced.

| MessageDecoding | Upstream | Behavior |
| --- | --- | --- |
| TypeScript (default) | `query()` in `@anthropic-ai/claude-agent-sdk` | Yields every message, consuming only `keep_alive` and host-only `session_state_changed`. A message of an unknown type, or one whose values do not fit its Rust struct, is Message::Unknown; a block that does not fit is ContentBlock::Unknown. Content never ends the stream. |
| Python | `parse_message` in `claude-agent-sdk` | Raises a MessageParseError, with Python's text and the message as data, for a known message missing a key Python reads. Skips a message of a type it does not parse and a block of a kind it does not parse. |

Python raises a TypeError, not a MessageParseError, when a value it indexes
is not a dictionary. Rust reports that as a MessageParseError whose text
names the field ("Field is not an object in assistant message: 'message'").

The corpus in `tests/fixtures/message_decoding/cases.json` is replayed through
the pinned TypeScript and Python `query()`; `mise run claude-sdk:message-goldens`
regenerates `expected.json` and `--check` compares it. `tests/message_decoding.rs`
holds both Rust modes to those goldens.
