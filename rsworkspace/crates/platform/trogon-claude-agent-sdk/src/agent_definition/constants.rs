/// The order a TypeScript caller writing the object literal sends: `description` and `prompt` first, then the rest
/// of the fields in declaration order. The SDK forwards the caller's object as is, so the order is the caller's.
pub(super) const TYPESCRIPT_ORDER: &[&str] = &[
    "description",
    "prompt",
    "tools",
    "disallowedTools",
    "model",
    "mcpServers",
    "criticalSystemReminder_EXPERIMENTAL",
    "skills",
    "initialPrompt",
    "maxTurns",
    "autoCompactWindow",
    "background",
    "omitClaudeMd",
    "memory",
    "effort",
    "permissionMode",
    "observer",
    "observerMessage",
];

/// The field order of the Python dataclass, which `asdict` keeps when it drops the `None` values.
pub(super) const PYTHON_ORDER: &[&str] = &[
    "description",
    "prompt",
    "tools",
    "disallowedTools",
    "model",
    "skills",
    "memory",
    "mcpServers",
    "initialPrompt",
    "maxTurns",
    "background",
    "effort",
    "permissionMode",
];
