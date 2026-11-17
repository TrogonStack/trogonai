use std::{
    cell::RefCell,
    io,
    path::{Path, PathBuf},
};
#[cfg(unix)]
use std::{ffi::OsStr, os::unix::ffi::OsStrExt};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{
    ArgvStyle, ClaudeAgentOptions, Error, ExtraArgument, ManagedSettingsDelivery, PluginDelivery, SandboxSettings,
    Settings, SettingsInput, SettingsLoadCauseError, UnicodeDecodeReason, adapters::ReadFile, argv::build_with,
    python_c_stack::PythonCStack,
};

const GOLDENS: &str = include_str!("../../tests/fixtures/settings_option/goldens.json");
const PYTHON_SETTINGS: &str = include_str!("../../tests/fixtures/settings_option/python_settings.json");
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
    build_with(options, None, files, PythonCStack::MODULE)
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
        Some(error) => assert_eq!(got, Err(error.to_owned()), "{name}"),
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
fn python_keeps_settings_numbers_as_json_loads_reads_them() {
    let options = ClaudeAgentOptions {
        argv_style: ArgvStyle::Python,
        settings: Some(SettingsInput::from_wire_string(
            r#"{"n": 123456789012345678901234567890, "m": -0, "f": 1.0, "e": 1E400, "x": NaN}"#,
        )),
        sandbox: sandbox("{}"),
        ..Default::default()
    };
    let argv = run(&options, &Files::new()).unwrap_or_default();
    assert!(
        argv.contains(
            &r#"{"n": 123456789012345678901234567890, "m": 0, "f": 1.0, "e": Infinity, "x": NaN, "sandbox": {}}"#
                .to_owned()
        ),
        "{argv:?}"
    );
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

#[test]
fn typescript_repeated_extra_arguments_collapse_before_settings_are_merged() {
    let options = typescript(ClaudeAgentOptions {
        extra_args: vec![extra("a", Some("1")), extra("b", None), extra("a", Some("2"))],
        settings: settings(r#"{"r":2}"#).map(SettingsInput::inline),
        ..Default::default()
    });
    let tokens = run(&options, &Files::new()).unwrap_or_default();
    assert_eq!(tokens.iter().filter(|token| *token == "--a").count(), 1);
    assert!(tokens.windows(3).any(|window| window == ["--a", "2", "--b"]));
}

/// The `--settings` value TypeScript passes for `settings` text merged with `sandbox`, or its error.
fn typescript_merged(settings: &str, sandbox_json: &str) -> Result<String, String> {
    let options = typescript(ClaudeAgentOptions {
        settings: Some(SettingsInput::json(settings)),
        sandbox: sandbox(sandbox_json),
        ..Default::default()
    });
    run(&options, &Files::new()).map(|argv| argv.last().cloned().unwrap_or_default())
}

#[test]
fn typescript_trims_settings_text_as_javascript_trims_it() {
    // `JSON.parse` refuses the byte order mark it keeps, so the document is read as empty.
    assert_eq!(
        typescript_merged("\u{feff}\u{a0}{\"a\":1}\u{2028}\u{3000}", r#"{"enabled":true}"#),
        Ok(r#"{"sandbox":{"enabled":true,"failIfUnavailable":true}}"#.to_owned())
    );
    assert_eq!(
        typescript_merged("\u{85}{\"a\":1}", r#"{"enabled":true}"#),
        Err("Cannot use both a settings file path and the sandbox option. Include the sandbox configuration in your settings file instead.".to_owned())
    );
}

#[test]
fn typescript_reads_settings_numbers_past_the_double_range_as_json_parse_does() {
    assert_eq!(
        typescript_merged(
            r#"{"n":1e400,"m":-1e400,"k":9223372036854775808,"sandbox":{"x":[1e400,2]}}"#,
            r#"{"enabled":true}"#
        ),
        Ok(
            r#"{"n":null,"m":null,"k":9223372036854776000,"sandbox":{"x":[null,2],"enabled":true,"failIfUnavailable":true}}"#
                .to_owned()
        )
    );
}

#[test]
fn a_sandbox_number_equal_to_an_infinity_marker_keeps_its_value() {
    assert_eq!(
        typescript_merged(
            r#"{"sandbox":{"network":{"httpProxyPort":1}}}"#,
            r#"{"enabled":true,"network":{"httpProxyPort":9223372036854775808.0,"x":[1.5,-4611686018427387904.0]}}"#
        ),
        Ok(
            r#"{"sandbox":{"network":{"httpProxyPort":9223372036854776000,"x":[1.5,-4611686018427388000]},"enabled":true,"failIfUnavailable":true}}"#
                .to_owned()
        )
    );
}

#[test]
fn python_strips_settings_text_as_str_strip_does() {
    let options = |settings: &str| ClaudeAgentOptions {
        argv_style: ArgvStyle::Python,
        settings: Some(SettingsInput::from_wire_string(settings)),
        sandbox: sandbox(r#"{"enabled":true}"#),
        ..Default::default()
    };
    let argv = run(&options("\u{1c}{\"a\": 1}\u{1f}"), &Files::new()).unwrap_or_default();
    assert!(
        argv.contains(&r#"{"a": 1, "sandbox": {"enabled": true}}"#.to_owned()),
        "{argv:?}"
    );
    let files = Files::new();
    assert!(run(&options("\u{feff}{\"a\": 1}"), &files).is_err());
    assert_eq!(*files.asked.borrow(), [PathBuf::from("\u{feff}{\"a\": 1}")]);
}

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn python_sandboxed(text: impl Into<String>) -> ClaudeAgentOptions {
    ClaudeAgentOptions {
        argv_style: ArgvStyle::Python,
        settings: Some(SettingsInput::from_wire_string(text)),
        sandbox: sandbox(r#"{"enabled":true}"#),
        ..Default::default()
    }
}

fn settings_value(argv: &[String]) -> Option<String> {
    let position = argv.iter().position(|token| token == "--settings")?;
    argv.get(position + 1).cloned()
}

#[test]
fn python_keeps_lone_surrogate_escapes_in_the_settings_value() -> TestResult {
    let goldens: Value = serde_json::from_str(PYTHON_SETTINGS)?;
    let pairs = goldens["surrogates"]
        .as_array()
        .ok_or("the goldens have no surrogate pairs")?;
    let mut expected = Vec::new();
    let mut actual = Vec::new();
    for pair in pairs {
        let text = pair[0].as_str().ok_or("a surrogate pair has no settings text")?;
        expected.push(pair[1].as_str().map(str::to_owned));
        actual.push(settings_value(&run(&python_sandboxed(text), &Files::new())?));
    }
    assert_eq!((pairs.len(), actual), (4, expected));
    Ok(())
}

#[test]
fn python_settings_nest_as_deep_as_json_loads_reads_them_in_a_plain_python_call() -> TestResult {
    let goldens: Value = serde_json::from_str(PYTHON_SETTINGS)?;
    let module = goldens["depth"]
        .as_array()
        .and_then(|entries| entries.iter().find(|entry| entry["caller"] == "module"))
        .ok_or("the goldens have no module depth")?;
    let loads = usize::try_from(module["loads"].as_u64().ok_or("the module depth has no loads")?)?;
    let nested = |depth: usize| format!("{}null{}", "{\"a\":".repeat(depth), "}".repeat(depth));
    let value = settings_value(&run(&python_sandboxed(nested(loads)), &Files::new())?).ok_or("no --settings value")?;
    let digest: String = Sha256::digest(value.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let refused = run(&python_sandboxed(nested(loads + 1)), &Files::new()).map(|argv| settings_value(&argv));
    let actual = (Some(digest), u64::try_from(value.len()).ok(), refused);
    let expected = (
        module["value_sha256"].as_str().map(str::to_owned),
        module["value_length"].as_u64(),
        Err(module["error"].as_str().unwrap_or_default().to_owned()),
    );
    assert_eq!(actual, expected);
    Ok(())
}

/// The bytes of a `files` entry of the Python goldens: hex before and after a run of `a` bytes.
fn golden_file(entry: &Value) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let head = hex_bytes(entry[0].as_str().ok_or("a settings file has no head")?)?;
    let pad = usize::try_from(entry[1].as_u64().ok_or("a settings file has no padding")?)?;
    let tail = hex_bytes(entry[2].as_str().ok_or("a settings file has no tail")?)?;
    Ok([head, vec![b'a'; pad], tail].concat())
}

fn hex_bytes(text: &str) -> Result<Vec<u8>, std::num::ParseIntError> {
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(text.get(at..at + 2).unwrap_or_default(), 16))
        .collect()
}

#[test]
fn python_reads_a_settings_file_as_read_text_with_utf_8_sig_does() -> TestResult {
    let goldens: Value = serde_json::from_str(PYTHON_SETTINGS)?;
    let entries = goldens["files"]
        .as_array()
        .ok_or("the goldens have no settings files")?;
    let mut expected = Vec::new();
    let mut actual = Vec::new();
    for entry in entries {
        let mut files = Files::new();
        files.contents = golden_file(entry)?;
        let text = entry[3][1].as_str().ok_or("a settings file has no outcome")?.to_owned();
        expected.push(if entry[3][0] == "value" {
            Ok(Some(text))
        } else {
            Err(text)
        });
        actual.push(run(&python_sandboxed("settings_file.json"), &files).map(|argv| settings_value(&argv)));
    }
    assert_eq!((entries.len(), actual), (31, expected));
    Ok(())
}

#[cfg(unix)]
#[test]
fn python_quotes_a_missing_settings_file_with_the_repr_of_its_fsdecoded_path() -> TestResult {
    let goldens: Value = serde_json::from_str(PYTHON_SETTINGS)?;
    let entries = goldens["missing_paths"]
        .as_array()
        .ok_or("the goldens have no missing paths")?;
    let mut expected = Vec::new();
    let mut actual = Vec::new();
    for entry in entries {
        let cwd = hex_bytes(entry[0].as_str().ok_or("a missing path has no cwd")?)?;
        expected.push(Err(entry[1]
            .as_str()
            .ok_or("a missing path has no message")?
            .to_owned()));
        let options = ClaudeAgentOptions {
            cwd: Some(PathBuf::from(OsStr::from_bytes(&cwd))),
            ..python_sandboxed("none.json")
        };
        actual.push(run(&options, &Files::new()).map(|argv| settings_value(&argv)));
    }
    assert_eq!((entries.len(), actual), (10, expected));
    Ok(())
}

/// The `__cause__` of the `CLIConnectionError` Python raises, as the name of its class, with the fields of a
/// `UnicodeDecodeError`; a `RecursionError` is not caught, so it is no settings load error.
fn load_cause(name: &str, contents: &[u8]) -> String {
    let mut files = Files::new();
    files.contents = contents.to_vec();
    match build_with(&python_sandboxed(name), None, &files, PythonCStack::MODULE) {
        Err(Error::SettingsLoad(error)) => match error.cause() {
            SettingsLoadCauseError::Os(_) => "OSError".to_owned(),
            SettingsLoadCauseError::UnicodeDecode(cause) => format!(
                "UnicodeDecodeError {} {:?} {} {} {:?}",
                cause.encoding(),
                cause.object(),
                cause.start(),
                cause.end(),
                cause.reason()
            ),
            SettingsLoadCauseError::JsonDecode(_) => "JSONDecodeError".to_owned(),
            SettingsLoadCauseError::Integer(_) => "ValueError".to_owned(),
            SettingsLoadCauseError::Type(_) => "TypeError".to_owned(),
        },
        Err(error) => format!("not a settings load error: {error}"),
        Ok(_) => "loaded".to_owned(),
    }
}

#[test]
fn a_python_settings_load_error_carries_the_exception_python_caught() {
    let many_digits = format!(r#"{{"n":{}}}"#, "1".repeat(4301));
    let deep = format!("{}{}", "[".repeat(10_000), "]".repeat(10_000));
    let actual = [
        load_cause("none.json", b""),
        load_cause("settings_file.json", b"\xef\xbb\xbf{\"x\":\"\xe2\x82"),
        load_cause("settings_file.json", b"{"),
        load_cause("settings_file.json", many_digits.as_bytes()),
        load_cause("settings_file.json", b"[]"),
        load_cause("settings_file.json", deep.as_bytes()),
    ];
    let expected = [
        "OSError".to_owned(),
        format!(
            "UnicodeDecodeError utf-8 {:?} 6 8 {:?}",
            b"{\"x\":\"\xe2\x82",
            UnicodeDecodeReason::UnexpectedEndOfData
        ),
        "JSONDecodeError".to_owned(),
        "ValueError".to_owned(),
        "TypeError".to_owned(),
        "not a settings load error: maximum recursion depth exceeded while decoding a JSON array from a unicode string"
            .to_owned(),
    ];
    assert_eq!(actual, expected);
}
