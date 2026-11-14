/// The declaration order of TypeScript `AgentDefinition`, which a caller writing every field in order sends.
pub(super) const TYPESCRIPT_ORDER: &[&str] = &[
    "description",
    "tools",
    "disallowedTools",
    "prompt",
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
