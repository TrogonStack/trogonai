use std::collections::HashSet;
use std::error::Error;

use trogon_sandbox_contract::{Digest, DigestError};

fn assert_valid(value: &str) -> Result<(), Box<dyn Error>> {
    let digest = Digest::new(value)?;
    assert_eq!(digest.as_str(), value);
    assert_eq!(digest.to_string(), value);
    assert_eq!(Digest::new(value.to_owned())?, digest);
    assert_eq!(value.parse::<Digest>()?, digest);
    assert_eq!(Digest::try_from(value.to_owned())?, digest);
    assert_eq!(String::from(digest.clone()), value);

    let json = serde_json::to_string(value)?;
    assert_eq!(serde_json::to_string(&digest)?, json);
    assert_eq!(serde_json::from_str::<Digest>(&json)?, digest);

    let digests = HashSet::from([digest.clone()]);
    assert!(digests.contains(&Digest::new(value)?));
    Ok(())
}

fn assert_invalid(value: &str, expected: DigestError) -> Result<(), Box<dyn Error>> {
    assert_eq!(Digest::new(value).err().as_ref(), Some(&expected));
    assert_eq!(Digest::new(value.to_owned()).err().as_ref(), Some(&expected));
    assert_eq!(value.parse::<Digest>().err().as_ref(), Some(&expected));
    assert_eq!(Digest::try_from(value.to_owned()).err().as_ref(), Some(&expected));

    let json = serde_json::to_string(value)?;
    let decoded = serde_json::from_str::<Digest>(&json);
    assert!(decoded.is_err(), "invalid digest decoded: {value:?}");
    if let Err(error) = decoded {
        assert_eq!(error.classify(), serde_json::error::Category::Data);
    }
    assert!(expected.source().is_none());
    Ok(())
}

#[test]
fn every_ascii_hash_byte_obeys_the_lowercase_hex_grammar() -> Result<(), Box<dyn Error>> {
    for byte in 0_u8..=127 {
        let hash = format!("{}{}{}", "0".repeat(31), char::from(byte), "f".repeat(32));
        let value = format!("sha256:{hash}");
        if b"0123456789abcdef".contains(&byte) {
            assert_valid(&value)?;
        } else {
            assert_invalid(&value, DigestError::InvalidHash)?;
        }
    }
    for hash in ["0".repeat(64), "f".repeat(64), "0123456789abcdef".repeat(4)] {
        assert_valid(&format!("sha256:{hash}"))?;
    }
    Ok(())
}

#[test]
fn the_algorithm_prefix_is_exact_and_required() -> Result<(), Box<dyn Error>> {
    for value in ["", "sha256", "sha256 ", "sha256/", "sha256："] {
        assert_invalid(value, DigestError::MissingPrefix)?;
    }
    for prefix in [
        "",
        "SHA256:",
        "Sha256:",
        "sha512:",
        "sha-256:",
        " sha256:",
        "\nsha256:",
        "\0sha256:",
        "ésha256:",
    ] {
        assert_invalid(&format!("{prefix}{}", "a".repeat(64)), DigestError::MissingPrefix)?;
    }
    assert_invalid(&format!("sha256:sha256:{}", "a".repeat(64)), DigestError::InvalidHash)?;
    Ok(())
}

#[test]
fn hash_length_and_utf8_boundaries_are_rejected() -> Result<(), Box<dyn Error>> {
    for length in [0, 1, 63, 65, 128] {
        assert_invalid(&format!("sha256:{}", "a".repeat(length)), DigestError::InvalidHash)?;
    }
    for hash in [
        "é".repeat(32),
        "界".repeat(21) + "a",
        "😀".repeat(16),
        "ａ".repeat(21) + "a",
        "é".to_owned() + &"a".repeat(62),
        "a".repeat(31) + "é" + &"a".repeat(31),
        "a".repeat(62) + "é",
        "a".repeat(61) + "a\u{0301}",
        "A".repeat(64),
        "a".repeat(63) + "F",
        "g".to_owned() + &"a".repeat(63),
        "a".repeat(63) + " ",
        "a".repeat(64) + "\n",
        "a".repeat(64) + "\0",
    ] {
        assert_invalid(&format!("sha256:{hash}"), DigestError::InvalidHash)?;
    }
    Ok(())
}

#[test]
fn serde_requires_a_validated_string_and_accepts_json_escapes() -> Result<(), Box<dyn Error>> {
    for json in ["null", "true", "0", "[]", "{}", r#"{"digest":"sha256:00"}"#] {
        let decoded = serde_json::from_str::<Digest>(json);
        assert!(decoded.is_err(), "decoded {json}");
        if let Err(error) = decoded {
            assert_eq!(error.classify(), serde_json::error::Category::Data);
        }
    }
    let escaped = format!(r#""\u0073ha256:\u0061{}""#, "0".repeat(63));
    let digest: Digest = serde_json::from_str(&escaped)?;
    assert_eq!(digest.as_str(), format!("sha256:a{}", "0".repeat(63)));

    let zeros = Digest::new(format!("sha256:{}", "0".repeat(64)))?;
    let ones = Digest::new(format!("sha256:{}", "1".repeat(64)))?;
    assert_ne!(zeros, ones);
    assert_eq!(HashSet::from([zeros, ones]).len(), 2);
    Ok(())
}
