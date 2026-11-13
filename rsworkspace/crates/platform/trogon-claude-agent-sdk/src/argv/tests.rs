use super::build;
use crate::{ArgvStyle, ClaudeAgentOptions, Customizations, ExtraArgument, SessionPersistence};

fn tokens(options: &ClaudeAgentOptions) -> Vec<String> {
    build(options)
        .unwrap_or_default()
        .iter()
        .map(|token| token.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn typescript_base_is_stream_json_without_optional_flags() {
    assert_eq!(
        tokens(&ClaudeAgentOptions::default()),
        [
            "--output-format",
            "stream-json",
            "--verbose",
            "--input-format",
            "stream-json"
        ]
    );
}

#[test]
fn python_base_puts_input_format_last_and_sends_an_empty_prompt() {
    let options = ClaudeAgentOptions {
        argv_style: ArgvStyle::Python,
        ..ClaudeAgentOptions::default()
    };
    assert_eq!(
        tokens(&options),
        [
            "--output-format",
            "stream-json",
            "--verbose",
            "--system-prompt",
            "",
            "--input-format",
            "stream-json"
        ]
    );
}

#[test]
fn resume_uses_equals_in_both_styles() {
    for style in [ArgvStyle::TypeScript, ArgvStyle::Python] {
        let options = ClaudeAgentOptions {
            argv_style: style,
            resume: Some("123e4567-e89b-42d3-a456-426614174000".into()),
            ..ClaudeAgentOptions::default()
        };
        assert!(tokens(&options).contains(&"--resume=123e4567-e89b-42d3-a456-426614174000".to_owned()));
    }
}

#[test]
fn persistence_bare_and_extra_args_trail_the_known_flags() {
    let options = ClaudeAgentOptions {
        session_persistence: SessionPersistence::Disabled,
        customizations: Customizations::Bare,
        extra_args: vec![ExtraArgument {
            flag: "x".into(),
            value: Some("y".into()),
        }],
        ..ClaudeAgentOptions::default()
    };
    assert_eq!(
        tokens(&options)[5..],
        ["--no-session-persistence", "--bare", "--x", "y"]
    );
}
