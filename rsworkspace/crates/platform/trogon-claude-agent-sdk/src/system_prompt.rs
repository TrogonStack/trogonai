//! The `systemPrompt` option in every form the upstream SDKs accept.
//!
//! TypeScript (`OD` in `sdk.mjs` 0.3.323) sends the prompt in the initialize request; Python
//! (`subprocess_cli.py` 0.2.181) puts it on the command line and sends only `excludeDynamicSections` and
//! `systemPromptSnapshot` in the initialize request.

use std::{ffi::OsString, path::PathBuf};

use crate::OptionError;

/// `{ type: 'custom', prompt, snapshot }`. TypeScript accepts one string or several; Python only one string.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CustomPrompt {
    pub prompt: Vec<String>,
    /// Whether the session keeps the prompt it recorded on its first request. `None` leaves it to the CLI.
    pub snapshot: Option<bool>,
}

/// `{ type: 'preset', preset: 'claude_code', append, excludeDynamicSections, snapshot }`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PresetPrompt {
    /// Sent whenever set, including the empty string.
    pub append: Option<String>,
    pub exclude_dynamic_sections: Option<bool>,
    pub snapshot: Option<bool>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum SystemPrompt {
    /// No `systemPrompt`: an empty prompt, as both upstream SDKs default to.
    #[default]
    Empty,
    /// A bare string.
    Custom(String),
    /// A bare `string[]`, sent as is. TypeScript only.
    Parts(Vec<String>),
    /// The `{ type: 'custom' }` form, which can also carry `snapshot`.
    CustomWith(CustomPrompt),
    /// The `claude_code` preset.
    ClaudeCode,
    /// The `claude_code` preset with an appended text.
    ClaudeCodeWithAppend(String),
    /// The `claude_code` preset with every field the object form carries.
    Preset(PresetPrompt),
    /// Python's `{ type: 'file', path }`, passed as `--system-prompt-file`. Python only.
    File(PathBuf),
}

/// What the TypeScript initialize request carries for the prompt.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct InitializePrompt<'a> {
    pub system_prompt: Option<Vec<&'a str>>,
    pub append_system_prompt: Option<&'a str>,
    pub snapshot: Option<bool>,
    pub exclude_dynamic_sections: Option<bool>,
}

impl SystemPrompt {
    /// TypeScript `OD`: a missing prompt becomes `""`, a string is wrapped by the initialize builder, and only the
    /// preset forms leave `systemPrompt` undefined.
    pub(crate) fn typescript_initialize(&self) -> InitializePrompt<'_> {
        match self {
            Self::Empty => InitializePrompt {
                system_prompt: Some(vec![""]),
                ..InitializePrompt::default()
            },
            Self::Custom(prompt) => InitializePrompt {
                system_prompt: Some(vec![prompt.as_str()]),
                ..InitializePrompt::default()
            },
            Self::Parts(parts) => InitializePrompt {
                system_prompt: Some(parts.iter().map(String::as_str).collect()),
                ..InitializePrompt::default()
            },
            Self::CustomWith(custom) => InitializePrompt {
                system_prompt: Some(custom.prompt.iter().map(String::as_str).collect()),
                snapshot: custom.snapshot,
                ..InitializePrompt::default()
            },
            Self::ClaudeCode => InitializePrompt::default(),
            Self::ClaudeCodeWithAppend(append) => InitializePrompt {
                append_system_prompt: Some(append.as_str()),
                ..InitializePrompt::default()
            },
            Self::Preset(preset) => InitializePrompt {
                append_system_prompt: preset.append.as_deref(),
                snapshot: preset.snapshot,
                exclude_dynamic_sections: preset.exclude_dynamic_sections,
                ..InitializePrompt::default()
            },
            // The file is a command-line flag the TypeScript dialect does not have; `argv` rejects it.
            Self::File(_) => InitializePrompt::default(),
        }
    }

    /// Python `_internal/client.py`: `excludeDynamicSections` only from a preset, `snapshot` from a preset or a
    /// custom object.
    pub(crate) fn python_initialize(&self) -> InitializePrompt<'_> {
        match self {
            Self::CustomWith(custom) => InitializePrompt {
                snapshot: custom.snapshot,
                ..InitializePrompt::default()
            },
            Self::Preset(preset) => InitializePrompt {
                snapshot: preset.snapshot,
                exclude_dynamic_sections: preset.exclude_dynamic_sections,
                ..InitializePrompt::default()
            },
            _ => InitializePrompt::default(),
        }
    }

    /// Python `_build_command`: the flag that carries the prompt. A prompt with other than exactly one text has no
    /// Python equivalent.
    pub(crate) fn python_argv(&self) -> Result<Vec<OsString>, OptionError> {
        let single = |parts: &[String]| match parts {
            [only] => Ok(vec!["--system-prompt".into(), only.into()]),
            _ => Err(OptionError::new(
                "system_prompt takes a single string with ArgvStyle::Python; a list of texts needs ArgvStyle::TypeScript.",
            )),
        };
        match self {
            Self::Empty => Ok(vec!["--system-prompt".into(), "".into()]),
            Self::Custom(prompt) => Ok(vec!["--system-prompt".into(), prompt.into()]),
            Self::Parts(parts) => single(parts),
            Self::CustomWith(custom) => single(&custom.prompt),
            Self::ClaudeCode => Ok(Vec::new()),
            Self::ClaudeCodeWithAppend(append) => Ok(vec!["--append-system-prompt".into(), append.into()]),
            Self::Preset(preset) => Ok(preset
                .append
                .as_ref()
                .map(|append| vec!["--append-system-prompt".into(), append.into()])
                .unwrap_or_default()),
            Self::File(path) => Ok(vec!["--system-prompt-file".into(), path.into()]),
        }
    }

    /// TypeScript has no `--system-prompt-file`, so a file prompt is refused rather than dropped.
    pub(crate) fn reject_for_typescript(&self) -> Result<(), OptionError> {
        match self {
            Self::File(_) => Err(OptionError::new(
                "A system prompt file needs ArgvStyle::Python; pass `system-prompt-file` through extra_args with ArgvStyle::TypeScript.",
            )),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
#[path = "system_prompt_tests.rs"]
mod tests;
