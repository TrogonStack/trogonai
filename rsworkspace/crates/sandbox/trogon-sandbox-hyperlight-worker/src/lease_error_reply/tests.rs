use std::error::Error;

use super::*;

#[test]
fn each_failure_stage_serializes_under_its_own_field() -> Result<(), Box<dyn Error>> {
    let provider = serde_json::to_string(&LeaseErrorReply::ProviderError("busy".to_owned()))?;
    assert_eq!(provider, r#"{"provider_error":"busy"}"#);
    let decode = serde_json::to_string(&LeaseErrorReply::DecodeError("bad".to_owned()))?;
    assert_eq!(decode, r#"{"decode_error":"bad"}"#);
    Ok(())
}
