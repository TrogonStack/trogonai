use std::{
    error::Error as StdError,
    io::{self, Write},
};

use serde::{Serialize, Serializer};

use super::ResumeWriter;
use crate::SessionError;

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
    use std::ffi::OsString;

    use super::secure_storage_config_dir;
    use crate::{Environment, SummaryDialect};
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
