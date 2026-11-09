# Claude Agent SDK public API

This crate is an initial API draft for review. It exposes configuration and
client lifecycle signatures without a Claude Code runtime. `connect` and `disconnect`
return `Error::NotImplemented`; they start no process and exchange no messages.

The interactive client follows Python's `ClaudeAgentOptions` and `ClaudeSDKClient`
terminology, with Rust acronym casing. Optional fields default to `None`
unless described otherwise. The TypeScript column refers to `Options` in the
pinned [TypeScript `0.3.319` declaration archive](https://registry.npmjs.org/@anthropic-ai/claude-agent-sdk/-/claude-agent-sdk-0.3.319.tgz).

| Rust field | Python field | TypeScript option |
| --- | --- | --- |
| `cli_path: Option<PathBuf>` | [`cli_path`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L2467) | `pathToClaudeCodeExecutable` |
| `cwd: Option<PathBuf>` | [`cwd`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L2464) | `cwd` |
| `model: Option<ModelName>` | [`model`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L2438) | `model` |
| `permission_mode: Option<PermissionMode>` | [`permission_mode`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L2393) | `permissionMode` |
| `setting_sources: Option<Vec<SettingSource>>` | [`setting_sources`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L2666) | `settingSources` |
| `max_turns: Option<NonZeroUsize>` | [`max_turns`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L2418) | `maxTurns` |
| `max_budget_usd: Option<UsdBudget>` | [`max_budget_usd`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L2424) | `maxBudgetUsd` |
| `effort: Option<EffortLevel>` | [`effort`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L2741) | `effort` |
| `env: Environment` | [`env`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L2492) | `env` |
| `additional_directories: Vec<PathBuf>` | [`add_dirs`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L2486) | `additionalDirectories` |
| `agent: Option<AgentName>` | Not exposed in pinned Python | `agent` |
| `allowed_tools: Vec<ToolRule>` | [`allowed_tools`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L2353) | `allowedTools` |

`PermissionMode` mirrors Python's
[`PermissionMode`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/types.py#L25)
and the exported type in
[TypeScript `0.3.319`](https://registry.npmjs.org/@anthropic-ai/claude-agent-sdk/-/claude-agent-sdk-0.3.319.tgz).
Serde uses their literal names: `default`, `acceptEdits`, `plan`,
`bypassPermissions`, `dontAsk`, and `auto`. `None` leaves the mode unset;
`Some(PermissionMode::Default)` explicitly selects `default`. The API draft
stores this choice without applying a permission policy.

`SettingSource` uses the exact serde literals `user`, `project`, and `local`.
Python's pinned `setting_sources` docs and TypeScript `0.3.319`'s `Options.settingSources`
docs both describe omission as loading all CLI sources and an empty list as
disabling filesystem settings. Rust preserves `None`, `Some(vec![])`, and caller
order without applying those choices in this API draft.

Model strings and paths are stored without extra identifier restrictions.
Python exposes [`connect`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/client.py#L93)
and [`disconnect`](https://github.com/anthropics/claude-agent-sdk-python/blob/2209f2c134fee914c0f75733b631a5fc238ca846/src/claude_agent_sdk/client.py#L607)
on an existing client. Rust intentionally uses an async factory for `connect`
and consumes the client in `disconnect(self)`. Published
[TypeScript `0.3.319`](https://www.npmjs.com/package/@anthropic-ai/claude-agent-sdk/v/0.3.319)
uses `Query` and `WarmQuery` with `close()` instead.

```rust
use std::path::PathBuf;
use trogon_claude_agent_sdk::{ClaudeAgentOptions, ClaudeSdkClient, Error};

# async fn example() -> Result<(), Error> {
let options = ClaudeAgentOptions {
    cli_path: Some(PathBuf::from("tools/claude")),
    cwd: Some(PathBuf::from(".")),
    model: Some("claude-sonnet-5".into()),
    ..ClaudeAgentOptions::default()
};
let client = ClaudeSdkClient::connect(options).await?;
client.disconnect().await?;
# Ok(())
# }
```

`max_turns` preserves omission separately from an explicit positive turn
limit. The Rust boundary uses `NonZeroUsize` for the configured count.

`max_budget_usd` distinguishes an omitted cap from an explicit zero.
`UsdBudget` admits finite, nonnegative amounts and preserves their value;
non-finite and negative amounts fail at construction with `UsdBudgetError`.

`EffortLevel` uses the upstream literals `low`, `medium`, `high`,
`xhigh`, and `max` through `as_str()`. An omitted effort remains distinct
from any selected level.

`env` defaults to an empty map and stores caller overrides without reading or
changing the process environment. Like Python, Rust stores string values; the
pinned TypeScript declaration additionally admits `undefined`. Each value is a
`ConfigValue`, which converts from a string as `Sensitive` so a value is only
shown when the caller marks it `Plain`. Debug output lists every variable name,
shows plain values, and prints sensitive values as `<redacted>`. `ModelName`
keeps the model out of debug output, so options debug output shows every other
setting without listing fields by hand.

`additional_directories` stores an ordered path list and defaults to
empty. Paths are not canonicalized, deduplicated, or restricted during
configuration assembly. The Rust field follows TypeScript
`additionalDirectories`; Python calls this option `add_dirs`.

`agent` follows the pinned TypeScript `Options.agent` selection field;
the reviewed Python options do not expose a counterpart. `AgentName` stores
the string unchanged without identifier restrictions during configuration
assembly. Process-start checks belong to the later runtime boundary. This draft
stores the selection without starting an agent.

`allowed_tools` stores ordered tool rules and defaults to empty.
`ToolRule` preserves rule patterns while refusing empty values, control
characters, and commas that could split a single rule at the CLI boundary.
Duplicate rules remain present; this draft applies no permission policy.

The example checks API composition; it does not run a session. Further options
and runtime behavior belong in separate review slices.
