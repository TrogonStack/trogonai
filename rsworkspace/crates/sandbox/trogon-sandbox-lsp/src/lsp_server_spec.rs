use serde_json::Value;
use trogon_sandbox_contract::CommandEnv;

use crate::lsp_preset::LspPreset;
use crate::lsp_preset_name::LspPresetName;

/// How to launch a language server and which language its documents are by default.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LspServerSpec {
    pub command: String,
    pub args: Vec<String>,
    pub env: CommandEnv,
    pub language_id: Option<String>,
    pub initialization_options: Option<Value>,
}

impl LspServerSpec {
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
            ..Self::default()
        }
    }

    pub fn args(mut self, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }

    pub fn language_id(mut self, language_id: impl Into<String>) -> Self {
        self.language_id = Some(language_id.into());
        self
    }
}

impl From<&LspPreset> for LspServerSpec {
    fn from(preset: &LspPreset) -> Self {
        Self::new(preset.command)
            .args(preset.args.iter().copied())
            .language_id(preset.language_id)
    }
}

impl From<LspPresetName> for LspServerSpec {
    fn from(name: LspPresetName) -> Self {
        name.preset().into()
    }
}
