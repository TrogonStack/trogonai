# Message data reference

These Rust declarations support public data composition. They do not decode
CLI frames or start a session. Message structs keep their existing Clone and
Debug derives; this layer adds no Serialize or Deserialize implementation to
Message, its payload structs or content enums. MessageOrigin has its existing
validated serde implementation.

The lifecycle remains an API draft: connect and disconnect return the typed
NotImplemented error. No process, wire parser or permission policy is added.

Comparison sources are pinned [Python types](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py)
and the integrity-verified [TypeScript 0.3.319 declarations](https://registry.npmjs.org/@anthropic-ai/claude-agent-sdk/-/claude-agent-sdk-0.3.319.tgz).
The table entries describe stored shapes and deliberate Rust representations;
they do not assert runtime parity or complete upstream union coverage.

## JSON objects

JsonObject is serde_json::Map<String, serde_json::Value>, used for object-valued
payloads and explicit extension maps. Python uses dict[str, Any] for fields such
as ToolUseBlock.input. TypeScript object and Record types have comparable
object-valued roles, while Any and unknown can also represent non-object data.
The Rust alias is not a claim that either SDK exports a type named JsonObject.
The generic serde_json object decoder rejects scalar and array roots while
retaining nested nulls, arrays and objects. This is not an SDK frame decoder.
