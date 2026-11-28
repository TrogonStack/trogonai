use std::error::Error;

use serde_json::json;
use trogon_sandbox_actor_agent::{BindLeaseRequest, BindingError, BindingState, LeaseBinding};
use trogon_sandbox_contract::{LeaseError, LeaseId};

#[tokio::test]
async fn an_unbound_agent_refuses_every_lease() {
    let state = BindingState::new();
    for lease in ["01J0", "01J1", ""] {
        let refused = state.resolve(&LeaseId::new(lease)).await;
        assert_eq!(refused, Err(BindingError::Unbound));
    }
    assert_eq!(state.exposed_port().await, None);
    let error = LeaseError::from(BindingError::Unbound);
    assert!(matches!(error, LeaseError::Backend { .. }));
}

#[tokio::test]
async fn a_bound_agent_serves_only_its_own_lease() -> Result<(), Box<dyn Error>> {
    let state = BindingState::new();
    let request: BindLeaseRequest = serde_json::from_value(json!({"lease_id": "01J0"}))?;
    state.bind(LeaseBinding::from(request)).await;
    assert!(state.resolve(&LeaseId::new("01J0")).await.is_ok());
    assert!(matches!(
        state.resolve(&LeaseId::new("01J1")).await,
        Err(BindingError::LeaseMismatch { .. })
    ));
    Ok(())
}

#[test]
fn a_bind_request_omits_defaulted_fields_on_the_wire() -> Result<(), Box<dyn Error>> {
    let request: BindLeaseRequest = serde_json::from_value(json!({"lease_id": "01J0"}))?;
    let wire = serde_json::to_value(&request)?;
    let object = wire.as_object().ok_or("a bind request serializes to an object")?;
    assert!(!object.contains_key("working_dir"));
    assert!(!object.contains_key("exposed_port"));
    assert!(!object.contains_key("entropy_seed"));
    assert_eq!(object.get("env"), Some(&json!({})));
    assert_eq!(serde_json::from_value::<BindLeaseRequest>(wire)?, request);
    Ok(())
}
