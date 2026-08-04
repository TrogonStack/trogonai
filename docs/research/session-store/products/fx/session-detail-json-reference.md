# Fx session detail JSON

Status: Beta output reference  
Reference version: Fx v0.3.64  
Command: `fx session <session-id> --json`

This document describes the logical session object emitted by Fx for an exact
session ID. It is intended for applications that need to inspect or display an
Fx session.

This is not the format stored under `~/.fx/sessions/`. Applications should not
write to that directory. The on-disk representation includes event logs,
checkpoints, commit boundaries, and recovery metadata that are intentionally
not exposed here.

## Reading a session

List sessions to obtain an ID:

```sh
fx sessions --json
```

Then request the full logical session:

```sh
fx session <session-id> --json
```

If a session ID could be confused with command syntax, use the exact-ID form:

```sh
fx session --id <session-id> --json
```

Fx writes one JSON object followed by a newline to standard output. A successful
response has `kind: "session_detail"`.

`fx session last --json` returns only the latest workspace session summary. Use
an exact session ID when the full `history` array is required.

## Example response

```json
{
  "kind": "session_detail",
  "id": "1780675200000-1234567890-abcd1234abcd1234",
  "created_at_ms": 1780675200000,
  "updated_at_ms": 1780675265000,
  "history_len": 1,
  "conversation_language": "en",
  "history": [
    {
      "kind": "assistant",
      "user": {
        "text": "Inspect the entry point",
        "images": []
      },
      "assistant": "The entry point is src/main.zig.",
      "execution": {
        "schema_version": 2,
        "tool_steps": [],
        "files": []
      }
    }
  ]
}
```

## Top-level object

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `kind` | string | yes | Always `"session_detail"` for a successful response. |
| `id` | string | yes | Fx session ID. Treat it as an opaque identifier. |
| `created_at_ms` | integer | yes | Session creation time in Unix milliseconds. |
| `updated_at_ms` | integer | yes | Time of the latest durable session update in Unix milliseconds. |
| `history_len` | non-negative integer | yes | Number of entries in `history`. |
| `conversation_language` | string | yes | Conversation language tag, such as `en`, `ja`, or `und-Latn`. |
| `history` | array | yes | Ordered session history, oldest entry first. |

`history_len` is expected to equal `history.length`.

The detail response does not currently include the workspace path, model,
reasoning effort, token totals, usage charges, title, or preview.

## History entries

Each item in `history` is one of four variants selected by its `kind` field:

- `assistant`
- `background_command`
- `interrupted`
- `compacted_summary`

### Assistant turn

An ordinary completed user and assistant exchange. `execution` is always
present, including when both of its arrays are empty.

```json
{
  "kind": "assistant",
  "user": {
    "text": "Read src/main.zig",
    "images": []
  },
  "assistant": "The file defines the Fx composition root.",
  "execution": {
    "schema_version": 2,
    "tool_steps": [],
    "files": []
  }
}
```

| Field | Type | Required |
| --- | --- | --- |
| `kind` | `"assistant"` | yes |
| `user` | [User turn](#user-turn) | yes |
| `assistant` | string | yes |
| `execution` | [Execution memory](#execution-memory) | yes |

### Background command turn

A command that continued outside the foreground agent turn.

```json
{
  "kind": "background_command",
  "user": {
    "text": "Start the development server",
    "images": []
  },
  "assistant": "The server is running.",
  "execution": {
    "schema_version": 2,
    "tool_steps": [],
    "files": []
  },
  "log_path": "/path/to/session/background/1.log",
  "expect_url": true,
  "url": "http://localhost:3000",
  "background_record_id": "00112233445566778899aabbccddeeff"
}
```

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `kind` | `"background_command"` | yes | History variant. |
| `user` | [User turn](#user-turn) | yes | Request that started the command. |
| `assistant` | string | no | Assistant text associated with the command. Omitted when absent. |
| `execution` | [Execution memory](#execution-memory) | no | May be omitted when empty; a present object with empty arrays, as in the example, is also valid. |
| `log_path` | string | yes | Path to the command log. Treat the path as local to the Fx installation. |
| `expect_url` | boolean | yes | Whether Fx expected the command to expose a server URL. |
| `url` | string or null | yes | Discovered server URL, if available. |
| `background_record_id` | string | no | Stable 16-byte record ID encoded as 32 lowercase hexadecimal characters. |

### Interrupted turn

A user turn that stopped before ordinary completion. The assistant text and
active tool call may both be null.

```json
{
  "kind": "interrupted",
  "user": {
    "text": "Inspect the repository",
    "images": []
  },
  "assistant": "I inspected the entry point.",
  "tool_call": {
    "id": "call_123",
    "name": "read_file",
    "arguments_json": "{\"path\":\"src/main.zig\"}"
  },
  "completed_tool_names": ["list_files"],
  "execution": {
    "schema_version": 2,
    "tool_steps": [],
    "files": []
  }
}
```

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `kind` | `"interrupted"` | yes | History variant. |
| `user` | [User turn](#user-turn) | yes | Original user request. |
| `assistant` | string or null | yes | Partial assistant response, if one was produced. |
| `tool_call` | [Interrupted tool call](#interrupted-tool-call) or null | yes | Tool call active at interruption, if any. |
| `completed_tool_names` | array of strings | yes | Tools completed before interruption. |
| `execution` | [Execution memory](#execution-memory) | no | May be omitted when empty; a present object with empty arrays, as in the example, is also valid. |

### Compacted summary

Fx may replace older history with a summary to control context size.

```json
{
  "kind": "compacted_summary",
  "summary": "The user is implementing session support...",
  "removed_turn_count": 18,
  "compaction_count": 1
}
```

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `kind` | `"compacted_summary"` | yes | History variant. |
| `summary` | string | yes | Summary of the removed history. |
| `removed_turn_count` | non-negative integer | yes | Number of history entries represented by the summary. |
| `compaction_count` | non-negative integer | yes | Number of compaction passes represented by the entry. |

## Common nested objects

### User turn

```json
{
  "text": "Explain this image",
  "images": [
    {
      "path": "/path/to/image.png",
      "media_type": "image/png"
    }
  ]
}
```

| Field | Type | Required |
| --- | --- | --- |
| `text` | string | yes |
| `images` | array of image attachments | yes |

An image attachment contains:

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `path` | string | yes | Local resolved image path. |
| `media_type` | string | yes | Media type such as `image/png` or `image/jpeg`. |

Session-internal image identifiers, hashes, and snapshot locators are not
included in this output.

### Execution memory

```json
{
  "schema_version": 2,
  "tool_steps": [
    {
      "assistant": "I will inspect the file.",
      "tool_calls": [
        {
          "id": "call_123",
          "name": "read_file",
          "arguments_json": "{\"path\":\"src/main.zig\"}",
          "provider_result": null
        }
      ],
      "tool_results": [
        {
          "tool_call_id": "call_123",
          "tool_name": "read_file",
          "status": "success",
          "output": "const std = @import(\"std\");",
          "output_bytes": 32,
          "stored_output_bytes": 32,
          "truncated": false,
          "provider_native": false,
          "created_at_ms": 1780675201000,
          "permission_feedback": []
        }
      ]
    }
  ],
  "files": [
    {
      "path": "src/main.zig",
      "new_path": null,
      "tool_call_id": "call_123",
      "tool_name": "read_file",
      "action": "read",
      "status": "success",
      "model_view_covers_full_file": true,
      "stale": false
    }
  ]
}
```

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `schema_version` | integer | yes | Currently `2`. This versions the nested execution object, not the top-level response. |
| `tool_steps` | array of tool execution steps | yes | Ordered model and tool activity. |
| `files` | array of file evidence | yes | Files read or changed during the turn. |

### Tool execution step

| Field | Type | Required |
| --- | --- | --- |
| `assistant` | string or null | yes |
| `tool_calls` | array of tool calls | yes |
| `tool_results` | array of tool results | yes |

### Tool call

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `id` | string | yes | Tool call identifier. |
| `name` | string | yes | Fx tool name. |
| `arguments_json` | string | yes | Serialized tool arguments. Parse this string separately when structured arguments are needed. |
| `provider_result` | string or null | yes | Provider-native result associated with the call, when applicable. |

### Interrupted tool call

The `tool_call` directly on an interrupted history entry has a smaller shape:

| Field | Type | Required |
| --- | --- | --- |
| `id` | string | yes |
| `name` | string | yes |
| `arguments_json` | string | yes |

### Tool result

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `tool_call_id` | string | yes | Matching tool call ID. |
| `tool_name` | string | yes | Fx tool name. |
| `status` | `"success"` or `"failure"` | yes | Persisted result status. |
| `output` | string | yes | Stored tool output or stored preview. |
| `output_handle` | string | no | Handle for separately stored output, when available. |
| `preview` | string | no | Explicit output preview, when available. |
| `output_bytes` | non-negative integer | yes | Original output size in bytes. |
| `stored_output_bytes` | non-negative integer | yes | Number of output bytes retained in the session representation. |
| `truncated` | boolean | yes | Whether the stored output is truncated. |
| `provider_native` | boolean | yes | Whether the result came from provider-native tool execution. |
| `created_at_ms` | integer | yes | Result creation time in Unix milliseconds. |
| `permission_feedback` | array of strings | yes | Permission feedback associated with the result. |
| `committed_file_presentation` | object | no | Structured presentation for a committed file change. |

Private command replay descriptors and process presentation state are not
included in this command output.

### File evidence

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `path` | string | yes | Original or affected file path. |
| `new_path` | string or null | yes | Destination path for rename or copy operations. |
| `tool_call_id` | string | yes | Tool call associated with the evidence. |
| `tool_name` | string | yes | Tool that produced the evidence. |
| `action` | string enum | yes | `read`, `write`, `edit`, `delete`, `rename`, `copy`, `search`, `list`, or `unknown`. |
| `status` | `"success"` or `"failure"` | yes | Operation result. |
| `model_view_covers_full_file` | boolean | yes | Whether the model-visible content covered the complete file. |
| `stale` | boolean | yes | Whether later operations made this evidence stale. |

### Committed file presentation

This optional object describes a file change in a display-oriented form.

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `path` | string | yes | Changed file path. |
| `kind` | `"added"` or `"edited"` | yes | Presentation kind. |
| `lines` | array | yes | Display lines for the change. |
| `additions` | non-negative integer | yes | Added-line count. |
| `deletions` | non-negative integer | yes | Deleted-line count. |
| `truncated` | boolean | yes | Whether the presentation was shortened. |
| `previous_content` | string or null | yes | Previous content when retained. |
| `after_content` | string or null | yes | Resulting content when retained. |
| `lifecycle_id` | object or null | yes | `{ "turn_id": integer, "call_id": string }` when available. |

Each item in `lines` contains:

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `kind` | string enum | yes | `context`, `addition`, `deletion`, `elision`, or `notice`. |
| `old_line` | non-negative integer or null | yes | Old-file line number. |
| `new_line` | non-negative integer or null | yes | New-file line number. |
| `text` | string | yes | Renderable line content. |

## Error response

When `--json` is active, command failures are also emitted as one JSON object:

```json
{
  "kind": "session",
  "error": "record not found",
  "code": "SessionNotFound"
}
```

| Field | Type | Required |
| --- | --- | --- |
| `kind` | `"session"` | yes |
| `error` | string | yes |
| `code` | string | yes |

Callers should also check the process exit status and should not infer success
from valid JSON alone.

## Embedded JSON Schema

The following Draft 2020-12 schema describes the current successful response.
It allows unknown properties so consumers remain compatible with additive beta
fields.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "urn:fx:schema:session-detail:beta",
  "title": "Fx session detail",
  "type": "object",
  "required": [
    "kind",
    "id",
    "created_at_ms",
    "updated_at_ms",
    "history_len",
    "conversation_language",
    "history"
  ],
  "properties": {
    "kind": { "const": "session_detail" },
    "id": { "type": "string", "minLength": 1 },
    "created_at_ms": { "type": "integer" },
    "updated_at_ms": { "type": "integer" },
    "history_len": { "type": "integer", "minimum": 0 },
    "conversation_language": {
      "type": "string",
      "minLength": 1,
      "maxLength": 24
    },
    "history": {
      "type": "array",
      "items": { "$ref": "#/$defs/historyTurn" }
    }
  },
  "additionalProperties": true,
  "$defs": {
    "historyTurn": {
      "oneOf": [
        { "$ref": "#/$defs/assistantTurn" },
        { "$ref": "#/$defs/backgroundCommandTurn" },
        { "$ref": "#/$defs/interruptedTurn" },
        { "$ref": "#/$defs/compactedSummaryTurn" }
      ]
    },
    "assistantTurn": {
      "type": "object",
      "required": ["kind", "user", "assistant", "execution"],
      "properties": {
        "kind": { "const": "assistant" },
        "user": { "$ref": "#/$defs/userTurn" },
        "assistant": { "type": "string" },
        "execution": { "$ref": "#/$defs/executionMemory" }
      },
      "additionalProperties": true
    },
    "backgroundCommandTurn": {
      "type": "object",
      "required": ["kind", "user", "log_path", "expect_url", "url"],
      "properties": {
        "kind": { "const": "background_command" },
        "user": { "$ref": "#/$defs/userTurn" },
        "assistant": { "type": "string" },
        "execution": { "$ref": "#/$defs/executionMemory" },
        "log_path": { "type": "string" },
        "expect_url": { "type": "boolean" },
        "url": { "type": ["string", "null"] },
        "background_record_id": {
          "type": "string",
          "pattern": "^[0-9a-f]{32}$"
        }
      },
      "additionalProperties": true
    },
    "interruptedTurn": {
      "type": "object",
      "required": [
        "kind",
        "user",
        "assistant",
        "tool_call",
        "completed_tool_names"
      ],
      "properties": {
        "kind": { "const": "interrupted" },
        "user": { "$ref": "#/$defs/userTurn" },
        "assistant": { "type": ["string", "null"] },
        "tool_call": {
          "oneOf": [
            { "$ref": "#/$defs/interruptedToolCall" },
            { "type": "null" }
          ]
        },
        "completed_tool_names": {
          "type": "array",
          "items": { "type": "string" }
        },
        "execution": { "$ref": "#/$defs/executionMemory" }
      },
      "additionalProperties": true
    },
    "compactedSummaryTurn": {
      "type": "object",
      "required": [
        "kind",
        "summary",
        "removed_turn_count",
        "compaction_count"
      ],
      "properties": {
        "kind": { "const": "compacted_summary" },
        "summary": { "type": "string" },
        "removed_turn_count": { "type": "integer", "minimum": 0 },
        "compaction_count": { "type": "integer", "minimum": 0 }
      },
      "additionalProperties": true
    },
    "userTurn": {
      "type": "object",
      "required": ["text", "images"],
      "properties": {
        "text": { "type": "string" },
        "images": {
          "type": "array",
          "items": { "$ref": "#/$defs/imageAttachment" }
        }
      },
      "additionalProperties": true
    },
    "imageAttachment": {
      "type": "object",
      "required": ["path", "media_type"],
      "properties": {
        "path": { "type": "string" },
        "media_type": { "type": "string" }
      },
      "additionalProperties": true
    },
    "executionMemory": {
      "type": "object",
      "required": ["schema_version", "tool_steps", "files"],
      "properties": {
        "schema_version": { "const": 2 },
        "tool_steps": {
          "type": "array",
          "items": { "$ref": "#/$defs/toolExecutionStep" }
        },
        "files": {
          "type": "array",
          "items": { "$ref": "#/$defs/fileEvidence" }
        }
      },
      "additionalProperties": true
    },
    "toolExecutionStep": {
      "type": "object",
      "required": ["assistant", "tool_calls", "tool_results"],
      "properties": {
        "assistant": { "type": ["string", "null"] },
        "tool_calls": {
          "type": "array",
          "items": { "$ref": "#/$defs/toolCall" }
        },
        "tool_results": {
          "type": "array",
          "items": { "$ref": "#/$defs/toolResult" }
        }
      },
      "additionalProperties": true
    },
    "toolCall": {
      "type": "object",
      "required": ["id", "name", "arguments_json", "provider_result"],
      "properties": {
        "id": { "type": "string" },
        "name": { "type": "string" },
        "arguments_json": { "type": "string" },
        "provider_result": { "type": ["string", "null"] }
      },
      "additionalProperties": true
    },
    "interruptedToolCall": {
      "type": "object",
      "required": ["id", "name", "arguments_json"],
      "properties": {
        "id": { "type": "string" },
        "name": { "type": "string" },
        "arguments_json": { "type": "string" }
      },
      "additionalProperties": true
    },
    "toolResult": {
      "type": "object",
      "required": [
        "tool_call_id",
        "tool_name",
        "status",
        "output",
        "output_bytes",
        "stored_output_bytes",
        "truncated",
        "provider_native",
        "created_at_ms",
        "permission_feedback"
      ],
      "properties": {
        "tool_call_id": { "type": "string" },
        "tool_name": { "type": "string" },
        "status": { "enum": ["success", "failure"] },
        "output": { "type": "string" },
        "output_handle": { "type": "string" },
        "preview": { "type": "string" },
        "output_bytes": { "type": "integer", "minimum": 0 },
        "stored_output_bytes": { "type": "integer", "minimum": 0 },
        "truncated": { "type": "boolean" },
        "provider_native": { "type": "boolean" },
        "created_at_ms": { "type": "integer" },
        "permission_feedback": {
          "type": "array",
          "items": { "type": "string" }
        },
        "committed_file_presentation": {
          "$ref": "#/$defs/committedFilePresentation"
        }
      },
      "additionalProperties": true
    },
    "fileEvidence": {
      "type": "object",
      "required": [
        "path",
        "new_path",
        "tool_call_id",
        "tool_name",
        "action",
        "status",
        "model_view_covers_full_file",
        "stale"
      ],
      "properties": {
        "path": { "type": "string" },
        "new_path": { "type": ["string", "null"] },
        "tool_call_id": { "type": "string" },
        "tool_name": { "type": "string" },
        "action": {
          "enum": [
            "read",
            "write",
            "edit",
            "delete",
            "rename",
            "copy",
            "search",
            "list",
            "unknown"
          ]
        },
        "status": { "enum": ["success", "failure"] },
        "model_view_covers_full_file": { "type": "boolean" },
        "stale": { "type": "boolean" }
      },
      "additionalProperties": true
    },
    "committedFilePresentation": {
      "type": "object",
      "required": [
        "path",
        "kind",
        "lines",
        "additions",
        "deletions",
        "truncated",
        "previous_content",
        "after_content",
        "lifecycle_id"
      ],
      "properties": {
        "path": { "type": "string" },
        "kind": { "enum": ["added", "edited"] },
        "lines": {
          "type": "array",
          "items": { "$ref": "#/$defs/committedFilePresentationLine" }
        },
        "additions": { "type": "integer", "minimum": 0 },
        "deletions": { "type": "integer", "minimum": 0 },
        "truncated": { "type": "boolean" },
        "previous_content": { "type": ["string", "null"] },
        "after_content": { "type": ["string", "null"] },
        "lifecycle_id": {
          "oneOf": [
            { "$ref": "#/$defs/toolLifecycleId" },
            { "type": "null" }
          ]
        }
      },
      "additionalProperties": true
    },
    "committedFilePresentationLine": {
      "type": "object",
      "required": ["kind", "old_line", "new_line", "text"],
      "properties": {
        "kind": {
          "enum": ["context", "addition", "deletion", "elision", "notice"]
        },
        "old_line": { "type": ["integer", "null"], "minimum": 0 },
        "new_line": { "type": ["integer", "null"], "minimum": 0 },
        "text": { "type": "string" }
      },
      "additionalProperties": true
    },
    "toolLifecycleId": {
      "type": "object",
      "required": ["turn_id", "call_id"],
      "properties": {
        "turn_id": { "type": "integer", "minimum": 0 },
        "call_id": { "type": "string" }
      },
      "additionalProperties": true
    }
  }
}
```

## Compatibility and safety

- This is a beta read contract. The successful top-level response does not yet
  carry its own `schema_version`.
- Consumers should branch on `kind`, validate the required fields they use, and
  ignore unknown fields.
- `arguments_json` is a string containing serialized arguments. It is not an
  embedded JSON object.
- Paths are local machine paths and may not exist on another device.
- Session output may contain prompts, assistant responses, tool arguments,
  tool results, source code, and local paths. Treat it as potentially sensitive.
- This command does not provide a supported session import or write API.
- To continue the session with Fx, pass the opaque ID to
  `fx resume <session-id>` rather than editing session storage.
