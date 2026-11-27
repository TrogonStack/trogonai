use trogon_sandbox_contract::LeaseError;

use super::*;

#[test]
fn a_denial_keeps_the_cancel_denial_shape() -> Result<(), Box<dyn std::error::Error>> {
    let denial = CancelOperationResponse::Denied(LeaseError::Backend {
        message: "denied".to_owned(),
    });
    assert_eq!(
        serde_json::to_value(OperationCapabilitiesReply::Denied(denial.clone()))?,
        serde_json::to_value(denial)?
    );
    Ok(())
}
