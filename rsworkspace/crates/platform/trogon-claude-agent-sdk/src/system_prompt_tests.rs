#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use super::{CustomPrompt, PresetPrompt, SystemPrompt};

fn texts(prompt: &SystemPrompt) -> Option<Vec<&str>> {
    prompt.typescript_initialize().system_prompt
}

#[test]
fn typescript_sends_an_empty_prompt_by_default_and_wraps_a_string() {
    assert_eq!(texts(&SystemPrompt::Empty), Some(vec![""]));
    assert_eq!(texts(&SystemPrompt::Custom("a".into())), Some(vec!["a"]));
    assert_eq!(
        texts(&SystemPrompt::Parts(vec!["a".into(), "b".into()])),
        Some(vec!["a", "b"])
    );
}

#[test]
fn typescript_preset_leaves_the_prompt_undefined_and_carries_the_flags() {
    assert_eq!(texts(&SystemPrompt::ClaudeCode), None);
    let preset = SystemPrompt::Preset(PresetPrompt {
        append: Some(String::new()),
        exclude_dynamic_sections: Some(false),
        snapshot: Some(true),
    });
    let sent = preset.typescript_initialize();
    assert_eq!(sent.system_prompt, None);
    assert_eq!(sent.append_system_prompt, Some(""));
    assert_eq!(sent.exclude_dynamic_sections, Some(false));
    assert_eq!(sent.snapshot, Some(true));
}

#[test]
fn custom_object_carries_its_snapshot_in_both_dialects() {
    let custom = SystemPrompt::CustomWith(CustomPrompt {
        prompt: vec!["p".into()],
        snapshot: Some(false),
    });
    assert_eq!(custom.typescript_initialize().snapshot, Some(false));
    assert_eq!(custom.python_initialize().snapshot, Some(false));
    assert_eq!(custom.python_initialize().system_prompt, None);
}

#[test]
fn python_argv_follows_the_command_builder() {
    let argv = |prompt: SystemPrompt| prompt.python_argv().unwrap();
    assert_eq!(argv(SystemPrompt::Empty), ["--system-prompt", ""]);
    assert_eq!(argv(SystemPrompt::Custom("a".into())), ["--system-prompt", "a"]);
    assert_eq!(
        argv(SystemPrompt::File(PathBuf::from("p.md"))),
        ["--system-prompt-file", "p.md"]
    );
    assert!(argv(SystemPrompt::ClaudeCode).is_empty());
    assert!(argv(SystemPrompt::Preset(PresetPrompt::default())).is_empty());
    assert_eq!(
        argv(SystemPrompt::Preset(PresetPrompt {
            append: Some(String::new()),
            ..PresetPrompt::default()
        })),
        ["--append-system-prompt", ""]
    );
    assert!(SystemPrompt::Parts(vec!["a".into(), "b".into()]).python_argv().is_err());
}

#[test]
fn a_prompt_file_is_refused_in_the_typescript_dialect() {
    assert!(SystemPrompt::File("p".into()).reject_for_typescript().is_err());
    assert!(SystemPrompt::Empty.reject_for_typescript().is_ok());
}
