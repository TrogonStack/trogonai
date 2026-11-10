use buffa::Enumeration as _;
use trogon_decider_runtime::PrincipalId;
use trogon_peer::CallerId;
use trogon_peer_service::PeerApplicationError;
use trogonai_proto::google::rpc::Code;

#[test]
fn principal_construction_failures_remain_sanitized_internal_faults() -> Result<(), Box<dyn std::error::Error>> {
    // Valid CallerId values cannot fail PrincipalId validation, so this defensive mapping requires private access.
    for invalid in ["", "principal-sensitive-marker\n"] {
        assert!(CallerId::new(invalid).is_err());
        let source = PrincipalId::new(invalid)
            .err()
            .ok_or("invalid principal unexpectedly accepted")?;
        let fault = super::application::<(), ()>(PeerApplicationError::Principal(source));
        assert_eq!(fault.status().code, Code::INTERNAL.to_i32());
        assert_eq!(fault.message(), "peer execution failed");
        assert!(fault.status().details.is_empty());
    }
    Ok(())
}
