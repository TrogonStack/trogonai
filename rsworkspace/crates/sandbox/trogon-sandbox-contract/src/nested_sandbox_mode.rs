use serde::{Deserialize, Serialize};

/// Whether the sandbox runs inside an unprivileged container, where a fresh `/proc` cannot be mounted on Linux. The `Weaker` mode makes the payload share the `/proc` of the surrounding container, so it can read the processes outside its own PID namespace.
///
/// It serializes as a JSON boolean, `true` for `Weaker`, so existing peers keep parsing it. An omitted value reads
/// as `Standard`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum NestedSandboxMode {
    #[default]
    Standard,
    Weaker,
}

impl NestedSandboxMode {
    pub const fn is_weaker(self) -> bool {
        matches!(self, Self::Weaker)
    }
}

impl From<bool> for NestedSandboxMode {
    fn from(flag: bool) -> Self {
        if flag { Self::Weaker } else { Self::Standard }
    }
}

impl From<NestedSandboxMode> for bool {
    fn from(value: NestedSandboxMode) -> Self {
        value.is_weaker()
    }
}
