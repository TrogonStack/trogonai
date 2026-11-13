use std::{
    error::Error as StdError,
    ffi::OsString,
    io::{self, Write},
};

use serde::{Serialize, Serializer};

use super::{
    AuthSources, ResumeWriter, copy_auth_files, redact_refresh_token, redact_refresh_token_ts,
    secure_storage_config_dir, strip_settings_for_resume, strip_settings_for_resume_ts,
};
use crate::{
    Environment, EnvironmentMode, SessionError, SummaryDialect,
    adapters::{NativeFileSystem, TokioProcessSpawner},
    test_support::FixedEnvironment,
};

type TestResult = Result<(), Box<dyn StdError>>;

struct RejectedValue;

impl Serialize for RejectedValue {
    fn serialize<S: Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
        Err(serde::ser::Error::custom("fixture serializer rejected the value"))
    }
}

#[test]
fn bounded_writer_flush_preserves_exact_capacity_and_overflow_preserves_bytes() -> TestResult {
    let mut writer = ResumeWriter::new(4);
    writer.write_all(b"seed")?;
    writer.flush()?;
    assert_eq!(writer.bytes, b"seed");
    assert!(!writer.exceeded);
    assert!(matches!(writer.write(b"!"), Err(error) if error.kind() == io::ErrorKind::FileTooLarge));
    assert_eq!(writer.bytes, b"seed");
    assert!(writer.exceeded);
    Ok(())
}

#[test]
fn encoder_preserves_a_serializer_failure_as_the_encoding_source() -> TestResult {
    let mut writer = ResumeWriter::new(32);
    let Err(error) = writer.encode(&RejectedValue) else {
        return Err("serializer failure was ignored".into());
    };
    assert!(matches!(&error, SessionError::Encoding(source) if source.is_data()));
    let source = StdError::source(&error).ok_or("encoding source was discarded")?;
    assert!(
        source
            .downcast_ref::<serde_json::Error>()
            .is_some_and(serde_json::Error::is_data)
    );
    assert!(!writer.exceeded);
    assert!(writer.bytes.is_empty());
    Ok(())
}

#[test]
fn encoder_distinguishes_exhausted_writer_budget_from_data_failure() {
    let mut writer = ResumeWriter::new(0);
    assert!(matches!(writer.encode(&"value"), Err(SessionError::ByteLimit)));
    assert!(writer.exceeded);
    assert!(writer.bytes.is_empty());
}

#[test]
fn secure_storage_config_dir_follows_the_typescript_fallback_chain_on_windows_only() {
    let typescript = Some(SummaryDialect::TypeScript);
    let configured = |pairs: &[(&str, &str)]| {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).into()))
            .collect::<Environment>()
    };
    let inherited = |pairs: &[(&str, &str)]| {
        pairs
            .iter()
            .map(|(key, value)| (OsString::from(key), OsString::from(value)))
            .collect::<crate::environment_plan::Inherited>()
    };
    type Pairs = &'static [(&'static str, &'static str)];
    let cases: [(Pairs, Pairs, &str); 6] = [
        (
            &[
                ("CLAUDE_SECURESTORAGE_CONFIG_DIR", "env-secure"),
                ("CLAUDE_CONFIG_DIR", "env-config"),
            ],
            &[("CLAUDE_SECURESTORAGE_CONFIG_DIR", "process-secure")],
            "env-secure",
        ),
        (
            &[("CLAUDE_CONFIG_DIR", "env-config")],
            &[("CLAUDE_SECURESTORAGE_CONFIG_DIR", "process-secure")],
            "process-secure",
        ),
        (
            &[("CLAUDE_CONFIG_DIR", "env-config")],
            &[("CLAUDE_CONFIG_DIR", "process-config")],
            "env-config",
        ),
        (&[], &[("CLAUDE_CONFIG_DIR", "process-config")], "process-config"),
        (
            &[("CLAUDE_SECURESTORAGE_CONFIG_DIR", "")],
            &[("CLAUDE_CONFIG_DIR", "process-config")],
            "",
        ),
        (&[], &[], ""),
    ];
    for (env, process, expected) in cases {
        assert_eq!(
            secure_storage_config_dir(true, typescript, &configured(env), &inherited(process), true),
            Some(OsString::from(expected))
        );
    }
    let env = configured(&[("CLAUDE_CONFIG_DIR", "env-config")]);
    let process = inherited(&[]);
    assert_eq!(secure_storage_config_dir(true, typescript, &env, &process, false), None);
    assert_eq!(
        secure_storage_config_dir(true, Some(SummaryDialect::Python), &env, &process, true),
        None
    );
    assert_eq!(secure_storage_config_dir(true, None, &env, &process, true), None);
    assert_eq!(secure_storage_config_dir(false, typescript, &env, &process, true), None);
}

/// Oracle: upstream TypeScript `Ytt` drops `refreshToken` only when JavaScript holds it truthy, and leaves a document
/// that is not an object as read.
#[test]
fn typescript_refresh_token_redaction_follows_javascript_truthiness() {
    for unchanged in [
        "[1]",
        r#"{"claudeAiOauth":{"accessToken":"a","refreshToken":0}}"#,
        r#"{"claudeAiOauth":{"accessToken":"a","refreshToken":""}}"#,
    ] {
        assert_eq!(redact_refresh_token_ts(unchanged.to_owned()), unchanged);
    }
    for token in ["1", "0.5", "[]", "{}", "true", r#""r""#] {
        let credentials = format!(r#"{{"claudeAiOauth":{{"accessToken":"a","refreshToken":{token}}}}}"#);
        assert_eq!(
            redact_refresh_token_ts(credentials),
            r#"{"claudeAiOauth":{"accessToken":"a"}}"#,
            "{token}"
        );
    }
}

/// Oracle: upstream TypeScript `_7` copies a file that does not parse as read, and drops `env.CLAUDE_CONFIG_DIR`
/// while keeping the rest of `env`.
#[test]
fn typescript_settings_strip_the_config_dir_and_keep_unparsable_files() {
    assert_eq!(strip_settings_for_resume_ts(b"not json".to_vec()), b"not json");
    assert_eq!(
        strip_settings_for_resume_ts(br#"{"env":{"CLAUDE_CONFIG_DIR":"/x","KEEP":"1"},"model":"m"}"#.to_vec()),
        br#"{"env":{"KEEP":"1"},"model":"m"}"#
    );
}

#[test]
fn python_redaction_and_settings_strip_leave_untouched_documents_as_read() {
    for unchanged in [&b"not json"[..], br#"{"claudeAiOauth":{"accessToken":"a"}}"#] {
        assert_eq!(redact_refresh_token(unchanged.to_vec()), unchanged);
    }
    for unchanged in [&b"not json"[..], b"[]"] {
        assert_eq!(strip_settings_for_resume(unchanged.to_vec()), unchanged);
    }
}

#[tokio::test]
async fn python_credentials_that_cannot_be_written_fail_the_resume() -> Result<(), Box<dyn StdError>> {
    let directory = tempfile::tempdir()?;
    let source = directory.path().join("source");
    std::fs::create_dir_all(&source)?;
    std::fs::write(
        source.join(".credentials.json"),
        br#"{"claudeAiOauth":{"accessToken":"a"}}"#,
    )?;
    let environment = FixedEnvironment {
        directory: directory.path().to_path_buf(),
        variables: [(OsString::from("HOME"), directory.path().as_os_str().to_owned())].into(),
    };
    let outcome = copy_auth_files(
        &NativeFileSystem,
        &directory.path().join("missing"),
        AuthSources {
            caller_config_dir: Some(&source),
            source_config_dir: &source,
            subprocess_environment: &Environment::default(),
            environment_mode: EnvironmentMode::default(),
            environment: &environment,
            process: &TokioProcessSpawner,
            keychain: false,
        },
    )
    .await;
    assert!(matches!(outcome, Err(SessionError::Io(error)) if error.kind() == io::ErrorKind::NotFound));
    Ok(())
}
