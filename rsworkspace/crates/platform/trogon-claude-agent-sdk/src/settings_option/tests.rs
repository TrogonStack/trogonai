use std::{
    cell::RefCell,
    io,
    path::{Path, PathBuf},
};

use serde_json::Value;

use crate::{
    ArgvStyle, ClaudeAgentOptions, ExtraArgument, ManagedSettingsDelivery, PluginDelivery, SandboxSettings, Settings,
    SettingsInput, adapters::ReadFile, argv::build_with,
};

const GOLDENS: &str = include_str!("../../tests/fixtures/settings_option/goldens.json");
const SETTINGS_FILE: &str = r#"{"permissions":{"allow":["Bash(ls)"]},"sandbox":{"enabled":false,"network":{"allowedDomains":["a.example"],"httpProxyPort":8080}},"éx":"ü"}"#;

/// Serves one settings file for `settings_file.json` and records every path asked for.
struct Files {
    asked: RefCell<Vec<PathBuf>>,
    contents: Vec<u8>,
}

impl Files {
    fn new() -> Self {
        Self {
            asked: RefCell::default(),
            contents: SETTINGS_FILE.as_bytes().to_vec(),
        }
    }
}

impl ReadFile for Files {
    type Error = io::Error;

    fn read_file(&self, path: &Path) -> Result<Vec<u8>, io::Error> {
        self.asked.borrow_mut().push(path.to_path_buf());
        if path.file_name().is_some_and(|name| name == "settings_file.json") {
            Ok(self.contents.clone())
        } else {
            Err(io::Error::from_raw_os_error(2))
        }
    }
}

fn golden(style: &str, name: &str) -> Value {
    let all: Value = serde_json::from_str(GOLDENS).unwrap_or(Value::Null);
    all[style][name].clone()
}

fn settings(text: &str) -> Option<Settings> {
    Settings::from_json(text).ok()
}

fn sandbox(text: &str) -> Option<SandboxSettings> {
    serde_json::from_str(text).ok()
}

fn extra(flag: &str, value: Option<&str>) -> ExtraArgument {
    ExtraArgument {
        flag: flag.into(),
        value: value.map(str::to_owned),
    }
}

fn typescript(options: ClaudeAgentOptions) -> ClaudeAgentOptions {
    ClaudeAgentOptions {
        argv_style: ArgvStyle::TypeScript,
        ..options
    }
}

fn run(options: &ClaudeAgentOptions, files: &Files) -> Result<Vec<String>, String> {
    build_with(options, None, files)
        .map(|argv| argv.iter().map(|token| token.to_string_lossy().into_owned()).collect())
        .map_err(|error| error.to_string())
}

fn expect_typescript(name: &str, options: ClaudeAgentOptions) {
    let want = golden("typescript", name);
    let got = run(&typescript(options), &Files::new());
    match want["error"].as_str() {
        Some(error) => assert_eq!(got, Err(error.to_owned()), "{name}"),
        None => {
            let want: Vec<String> = serde_json::from_value(want["argv"].clone()).unwrap_or_default();
            assert_eq!(got.map(|argv| argv[5..].to_vec()), Ok(want), "{name}");
        }
    }
}

fn expect_python(name: &str, options: ClaudeAgentOptions) {
    let want = golden("python", name);
    let options = ClaudeAgentOptions {
        argv_style: ArgvStyle::Python,
        cwd: Some("<scratch>".into()),
        ..options
    };
    let got = run(&options, &Files::new());
    match want["error"].as_str() {
        Some(error) => {
            let got = got.err().unwrap_or_default();
            let error = error.split(" Expecting ").next().unwrap_or(error);
            assert!(got.starts_with("Failed to load settings: "), "{name}: {got}");
            if error.contains("[Errno") {
                assert_eq!(got, format!("{error}"), "{name}");
            }
        }
        None => {
            let want: Vec<String> = serde_json::from_value(want["argv"].clone()).unwrap_or_default();
            assert_eq!(got, Ok(want), "{name}");
        }
    }
}

#[test]
fn typescript_passes_a_settings_path_or_text_through_unchanged() {
    expect_typescript(
        "settings_path",
        ClaudeAgentOptions {
            settings: Some(SettingsInput::path("settings_file.json")),
            ..Default::default()
        },
    );
    expect_typescript(
        "settings_json_string",
        ClaudeAgentOptions {
            settings: Some(SettingsInput::json(r#"{"a": 1, "b": [1, 2]}"#)),
            ..Default::default()
        },
    );
    expect_typescript(
        "settings_dash_value",
        ClaudeAgentOptions {
            settings: Some(SettingsInput::json("-weird")),
            ..Default::default()
        },
    );
}

#[test]
fn typescript_writes_a_settings_object_with_json_stringify_bytes() {
    expect_typescript(
        "settings_object",
        ClaudeAgentOptions {
            settings: settings(r#"{"permissions":{"allow":["Bash(ls)"]},"model":"xé \u0001","1":2,"a":1.0}"#)
                .map(SettingsInput::inline),
            ..Default::default()
        },
    );
}

#[test]
fn typescript_merges_the_sandbox_into_the_settings_document() {
    expect_typescript(
        "sandbox_alone",
        ClaudeAgentOptions {
            sandbox: sandbox(
                r#"{"enabled":true,"network":{"allowedDomains":["x.com"],"httpProxyPort":1},"filesystem":{"denyRead":["/a"]},"foo":"bar"}"#,
            ),
            ..Default::default()
        },
    );
    expect_typescript(
        "sandbox_plus_object",
        ClaudeAgentOptions {
            settings: settings(
                r#"{"sandbox":{"enabled":false,"network":{"deniedDomains":["d"],"httpProxyPort":9,"socksProxyPort":10},"filesystem":{"disabled":true,"denyRead":["/a","/b"]}},"z":1}"#,
            ).map(SettingsInput::inline),
            sandbox: sandbox(
                r#"{"network":{"allowedDomains":["x.com"],"deniedDomains":["d","e"]},"filesystem":{"denyRead":["/b","/c"]}}"#,
            ),
            ..Default::default()
        },
    );
    expect_typescript(
        "sandbox_plus_json_string",
        ClaudeAgentOptions {
            settings: Some(SettingsInput::json(r#"{"x":1}"#)),
            sandbox: sandbox(r#"{"enabled":true}"#),
            ..Default::default()
        },
    );
    expect_typescript(
        "sandbox_plus_bad_json",
        ClaudeAgentOptions {
            settings: Some(SettingsInput::json("{bad}")),
            sandbox: sandbox(r#"{"enabled":true}"#),
            ..Default::default()
        },
    );
    expect_typescript(
        "sandbox_enabled_fail",
        ClaudeAgentOptions {
            sandbox: sandbox(r#"{"enabled":true,"filesystem":{"allowWrite":["/t"]},"ripgrep":{"command":"rg"}}"#),
            ..Default::default()
        },
    );
}

#[test]
fn typescript_refuses_a_settings_file_path_with_a_sandbox() {
    expect_typescript(
        "sandbox_plus_path",
        ClaudeAgentOptions {
            settings: Some(SettingsInput::path("settings_file.json")),
            sandbox: sandbox(r#"{"enabled":true}"#),
            ..Default::default()
        },
    );
}

#[test]
fn typescript_orders_the_settings_entry_among_the_extra_arguments() {
    expect_typescript(
        "sandbox_extra_args_settings",
        ClaudeAgentOptions {
            extra_args: vec![extra("settings", Some(r#"{"q":1}"#)), extra("other", None)],
            sandbox: sandbox(r#"{"enabled":true}"#),
            ..Default::default()
        },
    );
    expect_typescript(
        "extra_args_settings_and_option",
        ClaudeAgentOptions {
            extra_args: vec![
                extra("first", Some("1")),
                extra("settings", Some(r#"{"q":1}"#)),
                extra("last", Some("-x")),
            ],
            settings: settings(r#"{"r":2}"#).map(SettingsInput::inline),
            ..Default::default()
        },
    );
}

#[test]
fn typescript_delivers_managed_settings_by_argv_or_initialize() {
    let managed = || settings(r#"{"permissions":{"allow":["Bash"]},"n":1}"#);
    expect_typescript(
        "managed_argv",
        ClaudeAgentOptions {
            managed_settings: managed(),
            ..Default::default()
        },
    );
    expect_typescript(
        "managed_argv_dash",
        ClaudeAgentOptions {
            managed_settings: settings("{}"),
            ..Default::default()
        },
    );
    let initialize = ClaudeAgentOptions {
        managed_settings: managed(),
        managed_settings_delivery: ManagedSettingsDelivery::Initialize,
        ..Default::default()
    };
    expect_typescript("managed_initialize", initialize.clone());
    expect_typescript(
        "managed_initialize_resume",
        ClaudeAgentOptions {
            managed_settings: settings(r#"{"n":1}"#),
            resume: Some("abc".into()),
            ..initialize.clone()
        },
    );
    expect_typescript(
        "managed_initialize_plugins",
        ClaudeAgentOptions {
            managed_settings: settings(r#"{"n":1}"#),
            plugin_delivery: PluginDelivery::Initialize,
            plugins: vec![crate::LocalPlugin::new("/p")],
            ..initialize
        },
    );
    expect_typescript(
        "managed_initialize_without_settings",
        ClaudeAgentOptions {
            managed_settings_delivery: ManagedSettingsDelivery::Initialize,
            ..Default::default()
        },
    );
}

#[test]
fn python_passes_settings_text_through_and_skips_an_empty_value() {
    expect_python(
        "settings_path",
        ClaudeAgentOptions {
            settings: Some(SettingsInput::from_wire_string("settings_file.json")),
            ..Default::default()
        },
    );
    expect_python(
        "settings_json_string",
        ClaudeAgentOptions {
            settings: Some(SettingsInput::from_wire_string(r#"{"a": 1, "b": [1, 2]}"#)),
            ..Default::default()
        },
    );
    expect_python(
        "settings_empty",
        ClaudeAgentOptions {
            settings: Some(SettingsInput::Json(String::new())),
            ..Default::default()
        },
    );
    expect_python(
        "dash_settings",
        ClaudeAgentOptions {
            settings: Some(SettingsInput::from_wire_string("-weird")),
            ..Default::default()
        },
    );
    expect_python(
        "extra_args_settings",
        ClaudeAgentOptions {
            settings: Some(SettingsInput::from_wire_string("settings_file.json")),
            extra_args: vec![extra("settings", Some(r#"{"q":1}"#))],
            ..Default::default()
        },
    );
}

#[test]
fn python_replaces_the_sandbox_key_and_dumps_with_json_dumps_bytes() {
    expect_python(
        "sandbox_alone",
        ClaudeAgentOptions {
            sandbox: sandbox(
                r#"{"enabled":true,"network":{"allowedDomains":["x.com"],"httpProxyPort":1},"filesystem":{"denyRead":["/a"]},"foo":"bar","u":"é "}"#,
            ),
            ..Default::default()
        },
    );
    expect_python(
        "sandbox_plus_json",
        ClaudeAgentOptions {
            settings: Some(SettingsInput::from_wire_string(
                r#"  {"x": 1, "sandbox": {"enabled": false, "k": 2}, "f": 1.0}  "#,
            )),
            sandbox: sandbox(r#"{"enabled":true}"#),
            ..Default::default()
        },
    );
    expect_python(
        "sandbox_empty_dict",
        ClaudeAgentOptions {
            sandbox: sandbox("{}"),
            ..Default::default()
        },
    );
}

#[test]
fn python_reads_a_settings_file_relative_to_the_working_directory() {
    let options = ClaudeAgentOptions {
        settings: Some(SettingsInput::from_wire_string("settings_file.json")),
        sandbox: sandbox(r#"{"enabled":true}"#),
        ..Default::default()
    };
    expect_python("sandbox_plus_path", options.clone());
    let files = Files::new();
    let with_cwd = ClaudeAgentOptions {
        argv_style: ArgvStyle::Python,
        cwd: Some("/work".into()),
        ..options
    };
    assert!(run(&with_cwd, &files).is_ok());
    assert_eq!(*files.asked.borrow(), [PathBuf::from("/work/settings_file.json")]);
}

#[test]
fn python_reports_a_settings_file_that_cannot_be_loaded() {
    let sandboxed = |text: &str| ClaudeAgentOptions {
        settings: Some(SettingsInput::from_wire_string(text)),
        sandbox: sandbox(r#"{"enabled":true}"#),
        ..Default::default()
    };
    expect_python("sandbox_plus_path_missing", sandboxed("nope.json"));
    expect_python("sandbox_plus_array_like", sandboxed("[1]"));
    expect_python("sandbox_plus_bad_json", sandboxed("{bad}"));
}

#[test]
fn a_utf8_byte_order_mark_is_dropped_from_a_python_settings_file() {
    let mut files = Files::new();
    files.contents = [b"\xef\xbb\xbf".as_slice(), br#"{"a":1}"#].concat();
    let options = ClaudeAgentOptions {
        argv_style: ArgvStyle::Python,
        settings: Some(SettingsInput::from_wire_string("settings_file.json")),
        sandbox: sandbox("{}"),
        ..Default::default()
    };
    let argv = run(&options, &files).unwrap_or_default();
    assert!(argv.contains(&r#"{"a": 1, "sandbox": {}}"#.to_owned()), "{argv:?}");
}

#[test]
fn a_python_settings_file_holding_a_non_object_raises_the_python_type_error() {
    let mut files = Files::new();
    files.contents = b"\"text\"".to_vec();
    let options = ClaudeAgentOptions {
        argv_style: ArgvStyle::Python,
        settings: Some(SettingsInput::from_wire_string("settings_file.json")),
        sandbox: sandbox("{}"),
        ..Default::default()
    };
    assert_eq!(
        run(&options, &files).err().as_deref(),
        Some("Failed to load settings: 'str' object does not support item assignment")
    );
}

#[test]
fn managed_settings_are_ignored_by_the_python_dialect() {
    let options = ClaudeAgentOptions {
        argv_style: ArgvStyle::Python,
        managed_settings: settings(r#"{"n":1}"#),
        managed_settings_delivery: ManagedSettingsDelivery::Initialize,
        ..Default::default()
    };
    let argv = run(&options, &Files::new()).unwrap_or_default();
    assert!(!argv.iter().any(|token| token.contains("managed-settings")));
    assert!(super::initialize_managed_settings(&options).is_ok_and(|value| value.is_none()));
}

#[test]
fn a_lone_dash_value_stays_a_separate_token() {
    let options = ClaudeAgentOptions {
        argv_style: ArgvStyle::TypeScript,
        extra_args: vec![extra("flag", Some("-"))],
        ..Default::default()
    };
    let argv = run(&options, &Files::new()).unwrap_or_default();
    assert_eq!(argv[5..], ["--flag", "-"]);
}
