use crate::ModelPurpose;

/// Safe operation classes. No command, prompt, argument or destination is logged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    Model(ModelPurpose),
    WasmTool,
    Shell,
    PolicyChange,
}
