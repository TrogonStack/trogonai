use super::{build, request_await_initialize};
use crate::{
    ArgvStyle, ClaudeAgentOptions, ContinueConversation, Customizations, ExtraArgument, ScriptRuntime,
    SessionPersistence,
};

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

fn resumed() -> ClaudeAgentOptions {
    ClaudeAgentOptions {
        continue_conversation: ContinueConversation::Yes,
        ..ClaudeAgentOptions::default()
    }
}

#[test]
fn await_initialize_follows_typescript_resume_rules() {
    let mut options = resumed();
    request_await_initialize(&mut options);
    assert_eq!(tokens(&options)[5..], ["--continue", "--await-initialize"]);

    let mut fresh = ClaudeAgentOptions::default();
    request_await_initialize(&mut fresh);
    assert!(fresh.extra_args.is_empty());

    let mut explicit_path = ClaudeAgentOptions {
        cli_path: Some("claude".into()),
        ..resumed()
    };
    request_await_initialize(&mut explicit_path);
    assert!(explicit_path.extra_args.is_empty());

    let mut python = ClaudeAgentOptions {
        argv_style: ArgvStyle::Python,
        ..resumed()
    };
    request_await_initialize(&mut python);
    assert!(python.extra_args.is_empty());
}

#[test]
fn await_initialize_is_skipped_when_an_sdk_url_is_present() {
    let mut extra = ClaudeAgentOptions {
        extra_args: vec![ExtraArgument {
            flag: "sdk-url=wss://host".into(),
            value: None,
        }],
        ..resumed()
    };
    request_await_initialize(&mut extra);
    assert_eq!(extra.extra_args.len(), 1);

    let mut runtime = ClaudeAgentOptions {
        script_runtime: ScriptRuntime {
            executable: None,
            arguments: vec!["--sdk-url=wss://host".into()],
        },
        ..resumed()
    };
    request_await_initialize(&mut runtime);
    assert!(runtime.extra_args.is_empty());
}
