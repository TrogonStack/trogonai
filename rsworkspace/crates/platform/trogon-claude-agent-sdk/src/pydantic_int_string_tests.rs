use super::*;

#[test]
fn accepts_what_pydantic_reads_as_an_integer() {
    for text in [
        "12", "0", "-0", "+5", " 7 ", "1.0", "1.000", "1_2", "0-1", "-0_0", "007",
    ] {
        assert_eq!(read_int(text), Ok(()), "{text:?}");
    }
}

#[test]
fn rejects_what_pydantic_cannot_read() {
    for text in [
        "", "-", "+", "a", "1.5", "1__2", "_1", "1_", "0x10", "1e3", "- 1", "1 2",
    ] {
        assert_eq!(read_int(text), Err(IntStringFailure::Parsing), "{text:?}");
    }
}

#[test]
fn separates_too_many_digits_from_a_bad_number() {
    let digits = "9".repeat(4300);
    assert_eq!(read_int(&digits), Ok(()));
    assert_eq!(read_int(&format!("{digits}9")), Err(IntStringFailure::Size));
    assert_eq!(read_int(&format!("-{digits}9")), Err(IntStringFailure::Size));
    assert_eq!(read_int(&format!("+{digits}9")), Err(IntStringFailure::Parsing));
    assert_eq!(read_int(&format!("0-{digits}9")), Err(IntStringFailure::Parsing));
}
