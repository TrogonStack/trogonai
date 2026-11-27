use super::*;

#[test]
fn recognises_the_guest_agent_errno_text() {
    assert!(is_missing_file_error(
        r#"{"data":null,"message":"Failed to open file: No such file or directory"}"#
    ));
    assert!(!is_missing_file_error("Permission denied"));
}

#[test]
fn vm_id_replies_accept_numbers_and_decimal_strings() -> Result<(), Box<dyn std::error::Error>> {
    let number: VmIdResponse = serde_json::from_str("201")?;
    let text: VmIdResponse = serde_json::from_str("\"201\"")?;
    assert_eq!(number.validated()?, text.validated()?);
    let signed: VmIdResponse = serde_json::from_str("\"-1\"")?;
    assert!(signed.validated().is_err());
    let small: VmIdResponse = serde_json::from_str("7")?;
    assert!(small.validated().is_err());
    Ok(())
}
