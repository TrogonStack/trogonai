use std::num::NonZeroU32;

use serde_json::Value;
use trogon_sandbox_contract::TerminalIdError;

use super::DispatchError;
use crate::{EntropyError, PreviewGrantError, PreviewTokenError};

fn frame(error: DispatchError) -> Result<Value, serde_json::Error> {
    serde_json::to_value(error.into_frame("req".to_string()))
}

fn code(error: DispatchError) -> Result<String, Box<dyn std::error::Error>> {
    let frame = frame(error)?;
    Ok(frame["error"]["code"].as_str().ok_or("no code")?.to_string())
}

#[test]
fn each_failure_has_its_stable_code() -> Result<(), Box<dyn std::error::Error>> {
    let serialize = serde_json::from_str::<Value>("{").err().ok_or("parsed")?;
    let probe = reqwest::Client::new().get("http://[").build().err().ok_or("built")?;
    let entropy = EntropyError::from(rand_core::Error::from(NonZeroU32::MIN));
    let cases = [
        (DispatchError::TenantMismatch, "tenant_mismatch"),
        (DispatchError::Serialize(serialize), "internal"),
        (
            DispatchError::PreviewGrant(PreviewGrantError::Token(PreviewTokenError::Malformed)),
            "preview_grant",
        ),
        (DispatchError::TerminalUnsupported, "unsupported"),
        (DispatchError::Entropy(entropy), "internal"),
        (DispatchError::Probe(probe), "internal"),
        (
            DispatchError::TerminalId(TerminalIdError::InvalidCharacters),
            "internal",
        ),
        (DispatchError::UnknownTool("nope".to_string()), "unknown_tool"),
    ];
    for (error, expected) in cases {
        assert_eq!(code(error)?, expected);
    }
    Ok(())
}
