/// Data describing how to launch a language server. Presets carry no behavior.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct LspPreset {
    pub command: &'static str,
    pub args: &'static [&'static str],
    pub language_id: &'static str,
}

impl LspPreset {
    pub const TYPESCRIPT: Self = Self {
        command: "typescript-language-server",
        args: &["--stdio"],
        language_id: "typescript",
    };

    pub const PYTHON: Self = Self {
        command: "pyright-langserver",
        args: &["--stdio"],
        language_id: "python",
    };
}
