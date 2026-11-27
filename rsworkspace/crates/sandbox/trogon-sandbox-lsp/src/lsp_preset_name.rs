use crate::lsp_preset::LspPreset;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum LspPresetName {
    TypeScript,
    Python,
}

impl LspPresetName {
    pub fn preset(self) -> &'static LspPreset {
        match self {
            Self::TypeScript => &LspPreset::TYPESCRIPT,
            Self::Python => &LspPreset::PYTHON,
        }
    }
}
