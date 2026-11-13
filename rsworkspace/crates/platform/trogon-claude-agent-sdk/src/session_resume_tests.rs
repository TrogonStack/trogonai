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
