use crate::ToolSelector;

/// A tool the sandbox's workload should run, with its arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSpec {
    pub selector: ToolSelector,
    pub args: Vec<String>,
}
