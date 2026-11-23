use serde_json::error::Category::Data;
use serde_json::json;
use trogon_sandbox_contract::{CapabilitySupport, EgressCapability, PlacementCapability};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn support_serializes_as_a_json_boolean() -> TestResult {
    for (support, spelling) in [
        (CapabilitySupport::Supported, "true"),
        (CapabilitySupport::Unsupported, "false"),
    ] {
        assert_eq!(serde_json::to_string(&support)?, spelling);
        assert_eq!(serde_json::from_str::<CapabilitySupport>(spelling)?, support);
    }
    Ok(())
}

#[test]
fn support_rejects_values_that_are_not_booleans() {
    for json in ["\"yes\"", "\"true\"", "1", "null", "[]", "{}"] {
        let actual = serde_json::from_str::<CapabilitySupport>(json)
            .map(|_| None)
            .unwrap_or_else(|error| Some(error.classify()));
        assert_eq!(actual, Some(Data), "{json}");
    }
}

#[test]
fn support_defaults_to_unsupported() -> TestResult {
    assert_eq!(CapabilitySupport::default(), CapabilitySupport::Unsupported);
    let placement: PlacementCapability = serde_json::from_value(json!({}))?;
    assert_eq!(placement.supports_native_passthrough, CapabilitySupport::Unsupported);
    let egress: EgressCapability = serde_json::from_value(json!({ "deny_all": true }))?;
    assert_eq!(egress.deny_all, CapabilitySupport::Supported);
    assert_eq!(egress.host_allowlist, CapabilitySupport::Unsupported);
    Ok(())
}

#[test]
fn support_converts_to_and_from_bool() {
    for supported in [true, false] {
        let support = CapabilitySupport::from(supported);
        assert_eq!(support.is_supported(), supported);
        assert_eq!(bool::from(support), supported);
    }
    assert!(CapabilitySupport::Supported.is_supported());
    assert!(!CapabilitySupport::Unsupported.is_supported());
}
