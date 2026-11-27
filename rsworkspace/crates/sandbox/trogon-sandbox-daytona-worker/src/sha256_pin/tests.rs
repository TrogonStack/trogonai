use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn a_lowercase_digest_is_accepted() -> TestResult {
    let pin = Sha256Pin::parse(&"ab".repeat(32))?;
    assert_eq!(pin.as_str(), "ab".repeat(32));
    Ok(())
}

#[test]
fn uppercase_short_long_and_non_hex_pins_are_refused() {
    for value in [
        "AB".repeat(32),
        "ab".repeat(31),
        "ab".repeat(33),
        "zz".repeat(32),
        String::new(),
    ] {
        assert!(matches!(
            Sha256Pin::parse(&value),
            Err(OrdinaryCommandRuntimeError::PinNotLowercaseHex)
        ));
    }
}

#[test]
fn the_digest_of_bytes_is_lowercase_hex() {
    assert_eq!(
        Sha256Pin::of(b"abc").as_str(),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}
