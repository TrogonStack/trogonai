use super::{build, request_await_initialize};
use crate::{
    ArgvStyle, ClaudeAgentOptions, ContinueConversation, Customizations, ExtraArgument, LocalPlugin, PluginDelivery,
    ScriptRuntime, SessionPersistence,
};

fn with_plugins(argv_style: ArgvStyle, delivery: PluginDelivery) -> ClaudeAgentOptions {
    ClaudeAgentOptions {
        argv_style,
        plugin_delivery: delivery,
        plugins: vec![
            LocalPlugin::new("/p/one"),
            LocalPlugin {
                path: "/p/two".into(),
                skip_mcp_discovery: Some(true),
            },
        ],
        ..ClaudeAgentOptions::default()
    }
}

#[test]
fn typescript_plugins_are_equals_form_flags_and_skip_mcp_selects_the_no_mcp_flag() {
    let all = tokens(&with_plugins(ArgvStyle::TypeScript, PluginDelivery::Argv));
    assert!(
        all.windows(2)
            .any(|pair| pair == ["--plugin-dir=/p/one", "--plugin-dir-no-mcp=/p/two"])
    );
}

#[test]
fn typescript_initialize_delivery_replaces_the_flags_with_await_initialize() {
    let all = tokens(&with_plugins(ArgvStyle::TypeScript, PluginDelivery::Initialize));
    assert!(all.contains(&"--await-initialize".to_owned()));
    assert!(!all.iter().any(|token| token.starts_with("--plugin-dir")));
}

#[test]
fn initialize_delivery_without_plugins_stays_on_argv() {
    let options = ClaudeAgentOptions {
        plugin_delivery: PluginDelivery::Initialize,
        ..ClaudeAgentOptions::default()
    };
    assert!(!tokens(&options).contains(&"--await-initialize".to_owned()));
}

#[test]
fn python_plugins_are_two_token_flags_and_ignore_the_delivery() {
    let all = tokens(&with_plugins(ArgvStyle::Python, PluginDelivery::Initialize));
    assert!(
        all.windows(4)
            .any(|quad| quad == ["--plugin-dir", "/p/one", "--plugin-dir", "/p/two"])
    );
    assert!(!all.contains(&"--await-initialize".to_owned()));
}

fn tokens(options: &ClaudeAgentOptions) -> Vec<String> {
    build(options, None)
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

fn with_workload(workload: &str, extra: Vec<ExtraArgument>) -> ClaudeAgentOptions {
    ClaudeAgentOptions {
        workload: Some(workload.into()),
        extra_args: extra,
        ..ClaudeAgentOptions::default()
    }
}

fn flag(name: &str, value: Option<&str>) -> ExtraArgument {
    ExtraArgument {
        flag: name.into(),
        value: value.map(str::to_owned),
    }
}

#[test]
fn workload_is_appended_after_the_built_in_flags() {
    let all = tokens(&with_workload("cron", Vec::new()));
    assert_eq!(all[5..], ["--workload", "cron"]);
}

#[test]
fn workload_replaces_an_extra_argument_of_the_same_name_in_place() {
    let options = with_workload(
        "cron",
        vec![flag("foo", Some("x")), flag("workload", Some("old")), flag("bar", None)],
    );
    assert_eq!(tokens(&options)[5..], ["--foo", "x", "--workload", "cron", "--bar"]);
}

#[test]
fn an_empty_workload_sends_nothing_and_python_ignores_it() {
    assert_eq!(
        tokens(&with_workload("", vec![flag("workload", Some("old"))]))[5..],
        ["--workload", "old"]
    );
    let mut python = with_workload("cron", Vec::new());
    python.argv_style = ArgvStyle::Python;
    assert!(!tokens(&python).contains(&"--workload".to_owned()));
}

#[test]
fn workload_precedes_await_initialize_and_an_existing_await_flag_becomes_bare_in_place() {
    let mut options = ClaudeAgentOptions {
        workload: Some("cron".into()),
        ..resumed()
    };
    request_await_initialize(&mut options);
    assert_eq!(
        tokens(&options)[5..],
        ["--continue", "--workload", "cron", "--await-initialize"]
    );

    let mut explicit = ClaudeAgentOptions {
        extra_args: vec![flag("await-initialize", Some("x")), flag("tail", None)],
        ..resumed()
    };
    request_await_initialize(&mut explicit);
    assert_eq!(tokens(&explicit)[5..], ["--continue", "--await-initialize", "--tail"]);
}
