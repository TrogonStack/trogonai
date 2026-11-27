use serde_json::json;

use super::*;

const OWNER: Owner<'static> = Owner {
    sandbox_id: "sbx-1",
    lease_id: "lease-1",
    organization_id: "org-1",
};

fn body(overrides: Value) -> Vec<u8> {
    let mut record = json!({
        "id": "sbx-1",
        "organizationId": "org-1",
        "labels": {"trogon.lease_id": "lease-1"},
        "networkBlockAll": true,
    });
    if let (Some(record), Some(overrides)) = (record.as_object_mut(), overrides.as_object()) {
        record.extend(overrides.clone());
    }
    record.to_string().into_bytes()
}

#[test]
fn an_owned_record_reports_field_shapes_only() {
    let observed = parse_owned(200, &body(json!({"networkAllowList": "10.0.0.0/8"})), &OWNER);
    assert_eq!(
        observed,
        Observation::OwnedNativePresent {
            network_block_all: Field::BooleanTrue,
            network_allow_list: Field::StringNonempty,
        }
    );
    assert!(!format!("{observed:?}").contains("10.0.0.0"));
}

#[test]
fn every_value_shape_has_a_name() {
    let cases = [
        (None, Field::Missing),
        (Some(json!(null)), Field::Null),
        (Some(json!("")), Field::StringEmpty),
        (Some(json!([])), Field::ArrayEmpty),
        (Some(json!(["a"])), Field::ArrayStrings),
        (Some(json!(["a", 1])), Field::ArrayMixed),
        (Some(json!(3)), Field::Number),
        (Some(json!({})), Field::Object),
        (Some(json!(false)), Field::BooleanFalse),
    ];
    for (value, shape) in cases {
        assert_eq!(Field::of(value.as_ref()), shape);
    }
}

#[test]
fn identity_organization_and_label_mismatches_are_distinct() {
    assert_eq!(
        parse_owned(200, &body(json!({"id": "other"})), &OWNER),
        Observation::IdentityMismatch
    );
    assert_eq!(
        parse_owned(200, &body(json!({"organizationId": "other"})), &OWNER),
        Observation::OrganizationMismatch
    );
    assert_eq!(
        parse_owned(200, &body(json!({"labels": {"trogon.lease_id": "other"}})), &OWNER),
        Observation::LeaseLabelMismatch
    );
}

#[test]
fn statuses_and_bodies_that_cannot_be_read_are_named() {
    assert_eq!(parse_owned(404, b"", &OWNER), Observation::NativeAbsent);
    assert_eq!(parse_owned(500, b"", &OWNER), Observation::UnexpectedHttpStatus);
    assert_eq!(parse_owned(200, b"not json", &OWNER), Observation::JsonMalformed);
    assert_eq!(
        parse_owned(200, &vec![b' '; MAX_BODY_BYTES + 1], &OWNER),
        Observation::BodyTooLarge
    );
}

#[test]
fn a_report_is_complete_only_with_a_blocked_restricted_lease_and_no_strays() {
    let open = Observation::OwnedNativePresent {
        network_block_all: Field::BooleanFalse,
        network_allow_list: Field::Missing,
    };
    let blocked = Observation::OwnedNativePresent {
        network_block_all: Field::BooleanTrue,
        network_allow_list: Field::Missing,
    };
    let report = Report {
        allow_all_control: open,
        deny_all_restricted: blocked,
        unexpected_grant_count: 0,
    };
    assert!(report.complete());
    assert!(
        !Report {
            deny_all_restricted: open,
            ..report
        }
        .complete()
    );
    assert!(
        !Report {
            unexpected_grant_count: 1,
            ..report
        }
        .complete()
    );
}
