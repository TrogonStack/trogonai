use buffa::Message;
use serde_json::json;

use super::assert_json_codec;
use crate::model_access::provider_connections::v1alpha1::{ProviderEndpoint, ProviderModelOffering};

#[test]
fn provider_endpoint_round_trips_through_json_and_wire() {
    assert_json_codec::<ProviderEndpoint>(json!({
        "network": {"baseUrl": "https://api.example.com/v1"}
    }));
}

#[test]
fn provider_model_offering_round_trips_through_json_and_wire() {
    assert_json_codec::<ProviderModelOffering>(json!({
        "identifier": {"value": "model-1"},
        "providerRevision": "2026-01"
    }));
}

#[test]
fn truncated_provider_endpoint_fails_closed() {
    let wire = [0x0a, 0x0c, 0x0a, 0x04, b'h', b't'];

    assert!(ProviderEndpoint::decode_from_slice(&wire).is_err());
}
