use super::*;
use serde_json::json;

fn fixture_pool() -> WireSchemaPool {
    let descriptor = json!({"file":[{"syntax":"proto2","package":"example","enumType":[
        {"name":"State","value":[{"name":"UNSPECIFIED","number":0},{"name":"KNOWN","number":1}]}
    ],"messageType":[
        {"name":"Id","field":[{"name":"value","number":1,"type":"TYPE_STRING","label":"LABEL_REQUIRED"}]},
        {"name":"Project","field":[{"name":"id","number":1,"type":"TYPE_MESSAGE","typeName":".example.Id","label":"LABEL_REQUIRED"}]},
        {"name":"Grant","field":[
            {"name":"id","number":2,"type":"TYPE_MESSAGE","typeName":".example.Id","label":"LABEL_REQUIRED"},
            {"name":"project","number":3,"type":"TYPE_MESSAGE","typeName":".example.Project","label":"LABEL_REQUIRED"}
        ]},
        {"name":"Binding","field":[
            {"name":"grant","number":1,"type":"TYPE_MESSAGE","typeName":".example.Grant","label":"LABEL_REQUIRED"},
            {"name":"id","number":2,"type":"TYPE_MESSAGE","typeName":".example.Id","label":"LABEL_REQUIRED"}
        ]},
        {"name":"Context","field":[
            {"name":"binding","number":1,"type":"TYPE_MESSAGE","typeName":".example.Binding","label":"LABEL_REQUIRED"},
            {"name":"invocation","number":2,"type":"TYPE_MESSAGE","typeName":".example.Id","label":"LABEL_REQUIRED"}
        ]},
        {"name":"Empty"},
        {"name":"Parent","oneofDecl":[{"name":"scope"}],"field":[
            {"name":"root","number":1,"type":"TYPE_MESSAGE","typeName":".example.Empty","label":"LABEL_OPTIONAL","oneofIndex":0},
            {"name":"project","number":2,"type":"TYPE_MESSAGE","typeName":".example.Project","label":"LABEL_OPTIONAL","oneofIndex":0}
        ]},
        {"name":"Failure","field":[
            {"name":"kind","number":1,"type":"TYPE_ENUM","typeName":".example.State","label":"LABEL_OPTIONAL"},
            {"name":"reason","number":4,"type":"TYPE_ENUM","typeName":".example.State","label":"LABEL_OPTIONAL"}
        ]},
        {"name":"Resolve","field":[{"name":"enabled","number":5,"type":"TYPE_BOOL","label":"LABEL_OPTIONAL"}]},
        {"name":"Entry","options":{"mapEntry":true},"field":[
            {"name":"key","number":1,"type":"TYPE_STRING","label":"LABEL_OPTIONAL"},
            {"name":"value","number":2,"type":"TYPE_STRING","label":"LABEL_OPTIONAL"}
        ]},
        {"name":"Selector","field":[{"name":"labels","number":2,"type":"TYPE_MESSAGE","typeName":".example.Entry","label":"LABEL_REPEATED"}]}
    ]}]});
    WireSchemaPool::from_descriptor_set_json(&serde_json::to_vec(&descriptor).unwrap()).unwrap()
}

#[test]
fn rejects_unknown_field_before_ordinary_decode_can_discard_it() {
    let descriptor = context_descriptor();
    assert_eq!(
        preflight(&descriptor, &[0x18, 0x01], WireLimits::new(4096, 16, 256).unwrap()),
        Err(WirePreflightError::UnknownField { field_number: 3 })
    );
}

fn descriptor(name: &str) -> WireMessageDescriptor {
    fixture_pool().message_by_name(name).unwrap()
}

fn context_descriptor() -> WireMessageDescriptor {
    descriptor("example.Context")
}

fn ld(field: u8, content: &[u8]) -> Vec<u8> {
    assert!(content.len() < 128);
    let mut bytes = vec![(field << 3) | 2, content.len() as u8];
    bytes.extend_from_slice(content);
    bytes
}

fn value(text: &str) -> Vec<u8> {
    ld(1, text.as_bytes())
}

fn context_parts() -> (Vec<u8>, Vec<u8>) {
    let mut grant = ld(2, &value("grant"));
    grant.extend(ld(3, &ld(1, &value("project"))));
    let mut binding = ld(1, &grant);
    binding.extend(ld(2, &value("binding")));
    (ld(1, &binding), ld(2, &value("invocation")))
}

fn context() -> Vec<u8> {
    let (mut binding, invocation) = context_parts();
    binding.extend(invocation);
    binding
}

fn limits() -> WireLimits {
    WireLimits::new(4096, 16, 256).unwrap()
}

#[test]
fn accepts_field_order_and_legal_nonminimal_varints_without_reencoding() {
    let (binding, mut invocation) = context_parts();
    assert_eq!(preflight(&context_descriptor(), &context(), limits()), Ok(()));
    invocation.extend(binding.clone());
    assert_eq!(preflight(&context_descriptor(), &invocation, limits()), Ok(()));
    let mut alternate = vec![binding[0], binding[1] | 0x80, 0];
    alternate.extend_from_slice(&binding[2..]);
    alternate.extend(context_parts().1);
    assert_eq!(preflight(&context_descriptor(), &alternate, limits()), Ok(()));
}

#[test]
fn rejects_duplicate_singular_nested_unknown_and_missing_required_values() {
    let mut duplicate = context();
    duplicate.extend(context_parts().1);
    assert_eq!(
        preflight(&context_descriptor(), &duplicate, limits()),
        Err(WirePreflightError::DuplicateSingular { field_number: 2 })
    );
    let mut malformed = context_parts().0;
    malformed.extend(ld(2, &[0x10, 1]));
    assert_eq!(
        preflight(&context_descriptor(), &malformed, limits()),
        Err(WirePreflightError::UnknownField { field_number: 2 })
    );
    assert_eq!(
        preflight(&context_descriptor(), &context_parts().0, limits()),
        Err(WirePreflightError::MissingRequired { field_number: 2 })
    );
}

#[test]
fn rejects_wire_ambiguity_truncation_and_non_utf8_strings() {
    for (bytes, expected) in [
        (vec![0, 0], WirePreflightError::InvalidTag),
        (vec![0x08, 1], WirePreflightError::WrongWireType { field_number: 1 }),
        (vec![0x0a, 127], WirePreflightError::Truncated),
        (vec![0x80; 10], WirePreflightError::MalformedVarint),
        (vec![0x0b], WirePreflightError::UnsupportedGroup),
    ] {
        assert_eq!(preflight(&context_descriptor(), &bytes, limits()), Err(expected));
    }
    let mut bytes = context_parts().0;
    bytes.extend(ld(2, &ld(1, &[0xff])));
    assert_eq!(
        preflight(&context_descriptor(), &bytes, limits()),
        Err(WirePreflightError::InvalidUtf8 { field_number: 1 })
    );
}

#[test]
fn rejects_conflicting_oneof_unknown_enum_and_non_boolean_values() {
    let parent = descriptor("example.Parent");
    let mut bytes = ld(1, &[]);
    bytes.extend(ld(2, &ld(1, &value("project"))));
    assert_eq!(
        preflight(&parent, &bytes, limits()),
        Err(WirePreflightError::ConflictingOneof)
    );
    let failure = descriptor("example.Failure");
    assert_eq!(
        preflight(&failure, &[0x08, 1, 0x20, 127], limits()),
        Err(WirePreflightError::UnknownEnum { field_number: 4 })
    );
    let resolve = descriptor("example.Resolve");
    assert_eq!(
        preflight(&resolve, &[0x28, 2], limits()),
        Err(WirePreflightError::InvalidBoolean { field_number: 5 })
    );
}

#[test]
fn rejects_map_fields_until_duplicate_key_semantics_are_supported() {
    let selector = descriptor("example.Selector");
    assert_eq!(
        preflight(&selector, &ld(2, &[]), limits()),
        Err(WirePreflightError::UnsupportedMap)
    );
}

#[test]
fn counts_all_nested_fields_bytes_and_depth_before_decoding() {
    for (bound, expected) in [
        (WireLimits::new(1, 16, 256).unwrap(), WirePreflightError::TooLarge),
        (WireLimits::new(4096, 1, 256).unwrap(), WirePreflightError::DepthLimit),
        (WireLimits::new(4096, 16, 1).unwrap(), WirePreflightError::FieldLimit),
    ] {
        assert_eq!(preflight(&context_descriptor(), &context(), bound), Err(expected));
    }
}

#[test]
fn limits_cannot_disable_budgets_or_create_unbounded_profiles() {
    assert_eq!(WireLimits::new(0, 16, 256), Err(WireLimitsError::Bytes));
    assert_eq!(WireLimits::new(4096, 0, 256), Err(WireLimitsError::Depth));
    assert_eq!(WireLimits::new(4096, 16, 0), Err(WireLimitsError::Fields));
    assert_eq!(WireLimits::new(17 * 1024 * 1024, 16, 256), Err(WireLimitsError::Bytes));
    assert_eq!(WireLimits::new(4096, 65, 256), Err(WireLimitsError::Depth));
    assert_eq!(WireLimits::new(4096, 16, 65537), Err(WireLimitsError::Fields));
}
