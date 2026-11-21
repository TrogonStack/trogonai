use std::error::Error;
use std::fmt::{Debug, Display};
use std::str::FromStr;

use serde::Serialize;
use serde::de::DeserializeOwned;
use trogon_sandbox_contract::{DispatchAttemptId, OperationId, TokenError, WorkerId};

fn assert_valid<T>(
    value: &str,
    new: &impl Fn(String) -> Result<T, TokenError>,
    as_str: fn(&T) -> &str,
) -> Result<(), Box<dyn Error>>
where
    T: Clone
        + Debug
        + Display
        + Eq
        + Serialize
        + DeserializeOwned
        + FromStr<Err = TokenError>
        + TryFrom<String, Error = TokenError>
        + Into<String>,
{
    let id = new(value.to_owned())?;
    assert_eq!(as_str(&id), value);
    assert_eq!(id.to_string(), value);
    assert_eq!(value.parse::<T>()?, id);
    assert_eq!(T::try_from(value.to_owned())?, id);
    assert_eq!(Into::<String>::into(id.clone()), value);

    let json = serde_json::to_string(value)?;
    assert_eq!(serde_json::to_string(&id)?, json);
    assert_eq!(serde_json::from_str::<T>(&json)?, id);
    Ok(())
}

fn assert_invalid<T>(
    value: &str,
    expected: TokenError,
    new: &impl Fn(String) -> Result<T, TokenError>,
) -> Result<(), Box<dyn Error>>
where
    T: Debug + DeserializeOwned + FromStr<Err = TokenError> + TryFrom<String, Error = TokenError>,
{
    assert_eq!(new(value.to_owned()).err(), Some(expected));
    assert_eq!(value.parse::<T>().err(), Some(expected));
    assert_eq!(T::try_from(value.to_owned()).err(), Some(expected));

    let json = serde_json::to_string(value)?;
    let decoded = serde_json::from_str::<T>(&json);
    assert!(decoded.is_err(), "invalid token decoded: {value:?}");
    if let Err(error) = decoded {
        assert_eq!(error.classify(), serde_json::error::Category::Data);
    }
    assert!(expected.source().is_none());
    Ok(())
}

fn assert_identifier_contract<T>(
    new: impl Fn(String) -> Result<T, TokenError>,
    as_str: fn(&T) -> &str,
) -> Result<(), Box<dyn Error>>
where
    T: Clone
        + Debug
        + Display
        + Eq
        + Serialize
        + DeserializeOwned
        + FromStr<Err = TokenError>
        + TryFrom<String, Error = TokenError>
        + Into<String>,
{
    for byte in 0_u8..=127 {
        let value = char::from(byte).to_string();
        let allowed = matches!(byte, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-');
        if allowed {
            assert_valid(&value, &new, as_str)?;
        } else {
            assert_invalid(&value, TokenError::InvalidCharacter { index: 0 }, &new)?;
        }
    }

    assert_valid(
        "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_-",
        &new,
        as_str,
    )?;
    for length in [1, 127, 128] {
        assert_valid(&"a".repeat(length), &new, as_str)?;
    }
    assert_invalid("", TokenError::Empty, &new)?;
    assert_invalid(
        &"a".repeat(129),
        TokenError::TooLong {
            actual: 129,
            maximum: 128,
        },
        &new,
    )?;

    for value in ["é", "界", "😀", &"é".repeat(64)] {
        assert_invalid(value, TokenError::InvalidCharacter { index: 0 }, &new)?;
    }
    for value in ["a.b", "a b", "a/b", "a\nb", "a\0b", "a\u{0301}"] {
        assert_invalid(value, TokenError::InvalidCharacter { index: 1 }, &new)?;
    }
    assert_invalid(
        &("a".repeat(127) + "."),
        TokenError::InvalidCharacter { index: 127 },
        &new,
    )?;
    assert_invalid(
        &"é".repeat(65),
        TokenError::TooLong {
            actual: 130,
            maximum: 128,
        },
        &new,
    )?;

    for json in ["null", "true", "0", "[]", "{}", r#"{"id":"a"}"#] {
        assert!(serde_json::from_str::<T>(json).is_err(), "decoded {json}");
    }
    let escaped: T = serde_json::from_str(r#""\u0041\u005f\u002d\u0030""#)?;
    assert_eq!(as_str(&escaped), "A_-0");
    Ok(())
}

#[test]
fn worker_id_preserves_the_token_contract() -> Result<(), Box<dyn Error>> {
    assert_identifier_contract(WorkerId::new, WorkerId::as_str)
}

#[test]
fn dispatch_attempt_id_preserves_the_token_contract() -> Result<(), Box<dyn Error>> {
    assert_identifier_contract(DispatchAttemptId::new, DispatchAttemptId::as_str)
}

#[test]
fn operation_id_preserves_the_token_contract() -> Result<(), Box<dyn Error>> {
    assert_identifier_contract(OperationId::new, OperationId::as_str)
}
