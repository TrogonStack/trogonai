use serde_json::error::Category::Data;
use serde_json::json;
use trogon_sandbox_contract::{CorrelationId, EmptyObject, LeaseId, TenantId, TenantIdError};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn lease_and_correlation_ids_serialize_as_unchecked_json_strings() -> TestResult {
    for value in ["lease-1", "", "any value: / \n"] {
        let lease = LeaseId::new(value);
        assert_eq!(lease.as_str(), value);
        assert_eq!(lease.to_string(), value);
        assert_eq!(LeaseId::from(value.to_owned()), lease);
        assert_eq!(serde_json::to_value(&lease)?, json!(value));
        assert_eq!(serde_json::from_value::<LeaseId>(json!(value))?, lease);
        assert_eq!(String::from(lease), value);

        let correlation = CorrelationId::new(value);
        assert_eq!(correlation.as_str(), value);
        assert_eq!(correlation.to_string(), value);
        assert_eq!(CorrelationId::from(value.to_owned()), correlation);
        assert_eq!(serde_json::to_value(&correlation)?, json!(value));
        assert_eq!(serde_json::from_value::<CorrelationId>(json!(value))?, correlation);
        assert_eq!(String::from(correlation), value);
    }
    Ok(())
}

#[test]
fn ids_reject_values_that_are_not_strings() {
    for value in [json!(1), json!(null), json!(["lease-1"]), json!({})] {
        assert!(serde_json::from_value::<LeaseId>(value.clone()).is_err(), "{value}");
        assert!(
            serde_json::from_value::<CorrelationId>(value.clone()).is_err(),
            "{value}"
        );
        assert!(serde_json::from_value::<TenantId>(value.clone()).is_err(), "{value}");
    }
}

#[test]
fn tenant_id_rejects_invalid_values_on_construction_parsing_and_deserialization() -> TestResult {
    let too_long = "a".repeat(TenantId::MAX_LEN + 1);
    for (value, expected) in [
        ("", TenantIdError::Empty),
        (too_long.as_str(), TenantIdError::TooLong { max: TenantId::MAX_LEN }),
        ("acme\nteam", TenantIdError::ControlCharacter),
        ("acme\u{7f}", TenantIdError::ControlCharacter),
    ] {
        assert_eq!(TenantId::new(value).err(), Some(expected), "{value:?}");
        assert_eq!(value.parse::<TenantId>().err(), Some(expected), "{value:?}");
        assert_eq!(TenantId::try_from(value.to_owned()).err(), Some(expected), "{value:?}");
        let error = serde_json::from_value::<TenantId>(json!(value))
            .err()
            .ok_or("invalid tenant id deserialized")?;
        assert_eq!(error.classify(), Data, "{value:?}");
    }
    Ok(())
}

#[test]
fn tenant_id_accepts_valid_values() -> TestResult {
    let longest = "a".repeat(TenantId::MAX_LEN);
    for value in [
        "acme",
        "acme_team",
        "org:acme",
        "org/acme",
        "é-tenant",
        longest.as_str(),
    ] {
        let tenant = TenantId::new(value)?;
        assert_eq!(tenant.as_str(), value);
        assert_eq!(tenant.to_string(), value);
        assert_eq!(value.parse::<TenantId>()?, tenant);
        assert_eq!(TenantId::try_from(value.to_owned())?, tenant);
        assert_eq!(serde_json::to_value(&tenant)?, json!(value));
        assert_eq!(serde_json::from_value::<TenantId>(json!(value))?, tenant);
        assert_eq!(String::from(tenant), value);
    }
    Ok(())
}

#[test]
fn tenant_id_errors_describe_the_rule() {
    assert_eq!(TenantIdError::Empty.to_string(), "tenant id must not be empty");
    assert_eq!(
        TenantIdError::TooLong { max: 128 }.to_string(),
        "tenant id must be at most 128 bytes"
    );
    assert_eq!(
        TenantIdError::ControlCharacter.to_string(),
        "tenant id must not contain control characters"
    );
}

#[test]
fn empty_object_serializes_as_an_empty_json_object() -> TestResult {
    assert_eq!(serde_json::to_string(&EmptyObject::default())?, "{}");
    assert_eq!(serde_json::from_str::<EmptyObject>("{}")?, EmptyObject {});
    Ok(())
}

#[test]
fn empty_object_rejects_fields_and_non_objects() {
    for json in [r#"{"field":1}"#, "[]", "[1]", "\"\"", "null", "1"] {
        let actual = serde_json::from_str::<EmptyObject>(json)
            .map(|_| None)
            .unwrap_or_else(|error| Some(error.classify()));
        assert_eq!(actual, Some(Data), "{json}");
    }
}
