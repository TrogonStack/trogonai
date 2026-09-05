use buffa::bytes::Bytes;
use buffa::type_registry::TypeRegistry;
use buffa::{DecodeError, DecodeOptions, Enumeration, Message, MessageField, UnknownFields};
use serde_json::{Value, json};

use super::{assert_json_codec, assert_malformed, assert_proto_sequence, assert_wire_codec};
use crate::r#gen::elixirpb::{self, FileOptions, FileOptionsOwnedView};
use crate::r#gen::trogon::error::v1alpha1::field_options::{ValuePolicy, ValuePolicyView};
use crate::r#gen::trogon::error::v1alpha1::message_options::{
    HelpLink, HelpLinkOwnedView, MetadataEntry, MetadataEntryOwnedView, Template, TemplateOwnedView,
};
use crate::r#gen::trogon::error::v1alpha1::{
    self as error, Code, FieldOptions, FieldOptionsOwnedView, MessageOptions, MessageOptionsOwnedView, Visibility,
};

fn template_json() -> Value {
    json!({
        "domain": "scheduler.example", "reason": "LIMIT_REACHED", "message": "Capacity unavailable",
        "code": "RESOURCE_EXHAUSTED", "visibility": "VISIBILITY_PUBLIC",
        "helpLinks": [{"url": "https://example.com/limits", "description": "Quota limits"}],
        "metadata": [{"key": "limit", "value": "concurrent_jobs", "visibility": "VISIBILITY_PRIVATE"}]
    })
}

#[test]
fn error_template_preserves_rpc_detail_fields_and_repeated_annotations() {
    let template = assert_json_codec::<Template>(template_json());
    assert_eq!(template.code, Code::ResourceExhausted);
    assert_eq!(template.visibility, Visibility::Public);
    assert_eq!(template.help_links[0].url, "https://example.com/limits");
    assert_eq!(template.metadata[0].visibility, Visibility::Private);

    let help = assert_json_codec::<HelpLink>(json!({"url": "u", "description": "d"}));
    assert_wire_codec(b"\x0a\x01u\x12\x01d", &help);
    let metadata = assert_json_codec::<MetadataEntry>(json!({
        "key": "k", "value": "v", "visibility": "VISIBILITY_PRIVATE"
    }));
    assert_wire_codec(b"\x0a\x01k\x12\x01v\x18\x01", &metadata);
    let annotated = assert_json_codec::<MessageOptions>(json!({"template": template_json()}));
    assert_eq!(annotated.template, MessageField::some(template));

    let expected = Template {
        domain: "d".to_owned(),
        reason: "r".to_owned(),
        message: "m".to_owned(),
        code: Code::Unavailable.into(),
        visibility: Visibility::Public.into(),
        help_links: vec![help, HelpLink::default()],
        metadata: vec![metadata, MetadataEntry::default()],
    };
    assert_wire_codec(
        b"\x0a\x01d\x12\x01r\x1a\x01m\x20\x0e\x28\x02\x32\x06\x0a\x01u\x12\x01d\x32\x00\x3a\x08\x0a\x01k\x12\x01v\x18\x01\x3a\x00",
        &expected,
    );
}

#[test]
fn message_annotation_presence_and_repeated_occurrences_follow_protobuf_merge_rules() {
    let absent = assert_json_codec::<MessageOptions>(json!({}));
    let present = assert_json_codec::<MessageOptions>(json!({"template": {}}));
    assert!(!absent.template.is_set());
    assert!(present.template.is_set());
    assert_wire_codec(b"", &absent);
    assert_wire_codec(b"\x0a\x00", &present);
    let merged = MessageOptions {
        template: MessageField::some(Template {
            domain: "d".to_owned(),
            reason: "r".to_owned(),
            help_links: vec![HelpLink::default(), HelpLink::default()],
            ..Template::default()
        }),
    };
    assert_wire_codec(b"\x0a\x05\x0a\x01d\x32\x00\x0a\x05\x12\x01r\x32\x00", &merged);
    let mut reused = merged;
    reused.clear();
    assert_eq!(reused, absent);
    assert_eq!(reused.try_encode_to_vec().expect("cleared annotation"), b"");
}

#[test]
fn field_policy_retains_explicit_empty_values_and_last_wire_alternative() {
    let fallback = assert_json_codec::<FieldOptions>(json!({
        "visibility": "VISIBILITY_PRIVATE", "defaultValue": "fallback"
    }));
    let fixed = assert_json_codec::<FieldOptions>(json!({
        "visibility": "VISIBILITY_PUBLIC", "value": "fixed"
    }));
    assert_wire_codec(b"\x08\x01\x12\x08fallback", &fallback);
    assert_wire_codec(b"\x08\x02\x1a\x05fixed", &fixed);
    assert_wire_codec(b"\x12\x08fallback\x08\x02\x1a\x05fixed", &fixed);
    assert_wire_codec(b"\x1a\x05fixed\x08\x01\x12\x08fallback", &fallback);

    let empty_fallback = assert_json_codec::<FieldOptions>(json!({"defaultValue": ""}));
    let empty_fixed = assert_json_codec::<FieldOptions>(json!({"value": ""}));
    assert_wire_codec(b"\x12\x00", &empty_fallback);
    assert_wire_codec(b"\x1a\x00", &empty_fixed);
    assert_ne!(empty_fallback, FieldOptions::default());
    assert_ne!(empty_fixed, FieldOptions::default());
    let alias: FieldOptions = serde_json::from_value(json!({"default_value": "fallback"})).expect("field alias");
    assert_eq!(
        alias.value_policy,
        Some(ValuePolicy::DefaultValue("fallback".to_owned()))
    );
    for invalid in [
        json!({"defaultValue": "fallback", "value": "fixed"}),
        json!({"defaultValue": 3}),
        json!({"value": false}),
    ] {
        assert!(serde_json::from_value::<FieldOptions>(invalid).is_err());
    }
    let nulls: FieldOptions = serde_json::from_value(json!({"defaultValue": null, "value": null}))
        .expect("null oneof alternatives are absent");
    assert_eq!(nulls, FieldOptions::default());
}

#[test]
fn unknown_error_codes_and_visibility_survive_while_closed_enum_names_are_validated() {
    assert_json_codec::<Template>(json!({"code": 123, "visibility": 124}));
    assert_json_codec::<MetadataEntry>(json!({"visibility": -1}));
    assert_json_codec::<FieldOptions>(json!({"visibility": 125}));
    for invalid in [
        json!({"code": "OK"}),
        json!({"code": "FUTURE_CODE"}),
        json!({"code": 4294967296_u64}),
    ] {
        assert!(serde_json::from_value::<Template>(invalid).is_err());
    }
    assert!(serde_json::from_value::<FieldOptions>(json!({"visibility": "VISIBILITY_SECRET"})).is_err());
    assert!(serde_json::from_value::<MetadataEntry>(json!({"visibility": 4294967296_u64})).is_err());
    let reset: Template = serde_json::from_value(json!({
        "domain": null, "reason": null, "message": null, "code": null,
        "visibility": null, "help_links": null, "metadata": null
    }))
    .expect("null fields use protobuf defaults");
    assert_eq!(reset, Template::default());
}

#[test]
fn template_error_codes_match_the_canonical_rpc_numeric_space_without_ok() {
    let cases = [
        ("UNSPECIFIED", 0, Code::Unspecified),
        ("CANCELLED", 1, Code::Cancelled),
        ("UNKNOWN", 2, Code::Unknown),
        ("INVALID_ARGUMENT", 3, Code::InvalidArgument),
        ("DEADLINE_EXCEEDED", 4, Code::DeadlineExceeded),
        ("NOT_FOUND", 5, Code::NotFound),
        ("ALREADY_EXISTS", 6, Code::AlreadyExists),
        ("PERMISSION_DENIED", 7, Code::PermissionDenied),
        ("RESOURCE_EXHAUSTED", 8, Code::ResourceExhausted),
        ("FAILED_PRECONDITION", 9, Code::FailedPrecondition),
        ("ABORTED", 10, Code::Aborted),
        ("OUT_OF_RANGE", 11, Code::OutOfRange),
        ("UNIMPLEMENTED", 12, Code::Unimplemented),
        ("INTERNAL", 13, Code::Internal),
        ("UNAVAILABLE", 14, Code::Unavailable),
        ("DATA_LOSS", 15, Code::DataLoss),
        ("UNAUTHENTICATED", 16, Code::Unauthenticated),
    ];
    assert_eq!(Code::values().len(), cases.len());
    assert_proto_sequence(
        cases.iter().map(|&(_, _, code)| code).collect(),
        json!(cases.map(|(name, _, _)| name)),
    );
    for (name, number, code) in cases {
        assert_eq!(Code::from_proto_name(name), Some(code));
        assert_eq!(Code::from_i32(number), Some(code));
        assert_eq!(code.to_i32(), number);
        assert_eq!(serde_json::to_value(code).expect("named code"), json!(name));
        assert_eq!(serde_json::from_value::<Code>(json!(name)).expect("named code"), code);
        assert_eq!(
            serde_json::from_value::<Code>(json!(number)).expect("numeric code"),
            code
        );
    }
    assert_eq!(Code::default(), Code::Unspecified);
    assert_eq!(
        serde_json::from_value::<Code>(Value::Null).expect("unset code"),
        Code::Unspecified
    );
    assert_eq!(Code::from_proto_name("OK"), None);
    assert_eq!(Code::from_i32(17), None);
    for invalid in [
        json!("OK"),
        json!(-1),
        json!(17),
        json!(4294967296_u64),
        json!(-4294967296_i64),
    ] {
        assert!(serde_json::from_value::<Code>(invalid).is_err());
    }
}

#[test]
fn visibility_names_and_numbers_preserve_the_descriptor_exposure_contract() {
    let cases = [
        ("VISIBILITY_UNSPECIFIED", 0, Visibility::Unspecified),
        ("VISIBILITY_PRIVATE", 1, Visibility::Private),
        ("VISIBILITY_PUBLIC", 2, Visibility::Public),
    ];
    assert_eq!(Visibility::values().len(), cases.len());
    assert_proto_sequence(
        cases.iter().map(|&(_, _, visibility)| visibility).collect(),
        json!(cases.map(|(name, _, _)| name)),
    );
    for (name, number, visibility) in cases {
        assert_eq!(Visibility::from_proto_name(name), Some(visibility));
        assert_eq!(Visibility::from_i32(number), Some(visibility));
        assert_eq!(visibility.to_i32(), number);
        assert_eq!(serde_json::to_value(visibility).expect("named visibility"), json!(name));
        assert_eq!(
            serde_json::from_value::<Visibility>(json!(name)).expect("named visibility"),
            visibility
        );
        assert_eq!(
            serde_json::from_value::<Visibility>(json!(number)).expect("numeric visibility"),
            visibility
        );
    }
    assert_eq!(Visibility::default(), Visibility::Unspecified);
    assert_eq!(
        serde_json::from_value::<Visibility>(Value::Null).expect("unset visibility"),
        Visibility::Unspecified
    );
    assert_eq!(Visibility::from_proto_name("VISIBILITY_SECRET"), None);
    assert_eq!(Visibility::from_i32(3), None);
    for invalid in [
        json!("VISIBILITY_SECRET"),
        json!(-1),
        json!(3),
        json!(4294967296_u64),
        json!(-4294967296_i64),
    ] {
        assert!(serde_json::from_value::<Visibility>(invalid).is_err());
    }
}

#[test]
fn descriptor_registry_routes_options_by_extendee_number_and_fully_qualified_name() {
    let mut registry = TypeRegistry::new();
    error::register_types(&mut registry);
    elixirpb::register_types(&mut registry);
    let cases = [
        (
            "trogon.error.v1alpha1.message",
            "google.protobuf.MessageOptions",
            870012,
            json!({"template": template_json()}),
        ),
        (
            "trogon.error.v1alpha1.field",
            "google.protobuf.FieldOptions",
            870013,
            json!({"value": "fixed"}),
        ),
        (
            "elixirpb.file",
            "google.protobuf.FileOptions",
            1047,
            json!({"modulePrefix": "Example"}),
        ),
    ];
    for (name, extendee, number, expected) in cases {
        let entry = registry.json_ext_by_name(name).expect("named extension");
        assert_eq!(entry.number, number);
        assert_eq!(entry.extendee, extendee);
        assert_eq!(
            registry
                .json_ext_by_number(extendee, number)
                .expect("numbered extension")
                .full_name,
            name
        );
        let mut fields = UnknownFields::default();
        for field in (entry.from_json)(expected.clone(), number).expect("extension JSON") {
            fields.push(field);
        }
        assert_eq!((entry.to_json)(number, &fields).expect("extension JSON"), expected);
        assert!(
            registry
                .json_ext_by_number("google.protobuf.ServiceOptions", number)
                .is_none()
        );
    }
    assert_eq!(
        (error::MESSAGE.number(), error::MESSAGE.extendee()),
        (870012, "google.protobuf.MessageOptions")
    );
    assert_eq!(
        (error::FIELD.number(), error::FIELD.extendee()),
        (870013, "google.protobuf.FieldOptions")
    );
    assert_eq!(
        (elixirpb::FILE.number(), elixirpb::FILE.extendee()),
        (1047, "google.protobuf.FileOptions")
    );

    for (type_url, expected) in [
        (MessageOptions::TYPE_URL, json!({"template": template_json()})),
        (Template::TYPE_URL, template_json()),
        (
            HelpLink::TYPE_URL,
            json!({"url": "https://example.com/limits", "description": "Limits"}),
        ),
        (
            MetadataEntry::TYPE_URL,
            json!({"key": "limit", "value": "jobs", "visibility": "VISIBILITY_PUBLIC"}),
        ),
        (FieldOptions::TYPE_URL, json!({"defaultValue": "fallback"})),
        (FileOptions::TYPE_URL, json!({"modulePrefix": "Example"})),
    ] {
        let entry = registry.json_any_by_url(type_url).expect("registered option type");
        let wire = (entry.from_json)(expected.clone()).expect("registry JSON");
        assert_eq!((entry.to_json)(&wire).expect("registry wire"), expected);
        assert!((entry.to_json)(b"\x00").is_err());
        assert!((entry.from_json)(json!(false)).is_err());
    }
}

#[test]
fn elixir_module_override_retains_absent_empty_and_replaced_package_names() {
    let absent = assert_json_codec::<FileOptions>(json!({}));
    let empty = assert_json_codec::<FileOptions>(json!({"modulePrefix": ""}));
    let named = assert_json_codec::<FileOptions>(json!({"modulePrefix": "Example"}));
    assert_wire_codec(b"", &absent);
    assert_wire_codec(b"\x0a\x00", &empty);
    assert_wire_codec(b"\x0a\x03Old\x0a\x07Example", &named);
    assert_eq!(FileOptions::default().with_module_prefix("Example"), named);
    assert_ne!(absent, empty);
    let alias: FileOptions = serde_json::from_value(json!({"module_prefix": "Example"})).expect("field alias");
    assert_eq!(alias, named);
    let mut source = named.clone();
    let retained = FileOptionsOwnedView::from_owned(&source).expect("retained file options");
    source.clear();
    drop(source);
    assert_eq!(retained.module_prefix(), Some("Example"));
    assert_eq!(retained.to_owned_message(), named);
    assert_eq!(
        serde_json::to_value(retained.view()).expect("retained JSON"),
        json!({"modulePrefix": "Example"})
    );
    assert_eq!(retained.bytes().as_ref(), b"\x0a\x07Example");
    assert_eq!(retained.into_bytes().as_ref(), b"\x0a\x07Example");
}

#[test]
fn retained_error_template_survives_source_reuse_and_preserves_nested_details() {
    let mut template: Template = serde_json::from_value(template_json()).expect("template");
    let retained = TemplateOwnedView::from_owned(&template).expect("retained template");
    template.clear();
    assert_eq!(template, Template::default());
    assert_eq!(retained.domain(), "scheduler.example");
    assert_eq!(retained.reason(), "LIMIT_REACHED");
    assert_eq!(retained.message(), "Capacity unavailable");
    assert_eq!(retained.code(), Code::ResourceExhausted);
    assert_eq!(retained.visibility(), Visibility::Public);
    assert_eq!(retained.help_links().len(), 1);
    assert_eq!(retained.metadata().len(), 1);
    assert_eq!(
        serde_json::to_value(retained.view()).expect("retained JSON"),
        template_json()
    );
    assert_eq!(
        serde_json::to_value(retained.to_owned_message()).expect("owned JSON"),
        template_json()
    );
    assert_eq!(
        retained.bytes().len(),
        retained.to_owned_message().try_encode_to_vec().expect("wire").len()
    );
    let bytes = retained.into_bytes();
    let transferred = TemplateOwnedView::decode(bytes).expect("transferred template");
    assert_eq!(
        serde_json::to_value(transferred).expect("transferred JSON"),
        template_json()
    );

    let mut options = MessageOptions {
        template: MessageField::some(transferred_template()),
    };
    let retained = MessageOptionsOwnedView::from_owned(&options).expect("retained message options");
    options.clear();
    assert!(retained.template().is_set());
    assert_eq!(
        serde_json::to_value(retained.view()).expect("retained annotation"),
        json!({"template": template_json()})
    );
    assert_eq!(retained.to_owned_message().template.domain, "scheduler.example");
    let original = retained.bytes().clone();
    assert_eq!(retained.into_bytes(), original);
}

fn transferred_template() -> Template {
    serde_json::from_value(template_json()).expect("template")
}

#[test]
fn retained_help_and_metadata_own_strings_after_source_clear() {
    let mut help = HelpLink {
        url: "https://example.com/limits".to_owned(),
        description: "Limits".to_owned(),
    };
    let retained = HelpLinkOwnedView::from_owned(&help).expect("retained help");
    help.clear();
    assert_eq!(help, HelpLink::default());
    assert_eq!(retained.url(), "https://example.com/limits");
    assert_eq!(retained.description(), "Limits");
    let expected = json!({"url": "https://example.com/limits", "description": "Limits"});
    assert_eq!(serde_json::to_value(retained.view()).expect("help view"), expected);
    assert_eq!(
        serde_json::to_value(retained.to_owned_message()).expect("owned help"),
        expected
    );
    let original = retained.bytes().clone();
    assert_eq!(retained.into_bytes(), original);

    let mut metadata = MetadataEntry {
        key: "limit".to_owned(),
        value: "jobs".to_owned(),
        visibility: Visibility::Private.into(),
    };
    let retained = MetadataEntryOwnedView::from_owned(&metadata).expect("retained metadata");
    metadata.clear();
    assert_eq!(metadata, MetadataEntry::default());
    assert_eq!(retained.key(), "limit");
    assert_eq!(retained.value(), "jobs");
    assert_eq!(retained.visibility(), Visibility::Private);
    let expected = json!({"key": "limit", "value": "jobs", "visibility": "VISIBILITY_PRIVATE"});
    assert_eq!(serde_json::to_value(retained.view()).expect("metadata view"), expected);
    assert_eq!(
        serde_json::to_value(retained.to_owned_message()).expect("owned metadata"),
        expected
    );
    let original = retained.bytes().clone();
    assert_eq!(retained.into_bytes(), original);
}

#[test]
fn retained_field_policy_keeps_selected_alternative_and_original_wire() {
    for policy in [
        ValuePolicy::DefaultValue("fallback".to_owned()),
        ValuePolicy::Value("fixed".to_owned()),
    ] {
        let mut source = FieldOptions {
            visibility: Visibility::Private.into(),
            value_policy: Some(policy.clone()),
        };
        let retained = FieldOptionsOwnedView::from_owned(&source).expect("retained field options");
        source.clear();
        assert_eq!(source, FieldOptions::default());
        assert_eq!(retained.visibility(), Visibility::Private);
        match (policy, retained.value_policy()) {
            (ValuePolicy::DefaultValue(expected), Some(ValuePolicyView::DefaultValue(actual)))
            | (ValuePolicy::Value(expected), Some(ValuePolicyView::Value(actual))) => assert_eq!(expected, *actual),
            other => panic!("policy changed: {other:?}"),
        }
        assert_eq!(
            serde_json::to_value(retained.view()).expect("policy view"),
            serde_json::to_value(retained.to_owned_message()).expect("owned policy")
        );
        let original = retained.bytes().clone();
        assert_eq!(retained.into_bytes(), original);
    }
    let wire = b"\x12\x08fallback\x1a\x05fixed\xf8\x07\x01";
    let limit = DecodeOptions::new().with_max_message_size(wire.len() - 1);
    assert_eq!(
        FieldOptionsOwnedView::decode_with_options(Bytes::copy_from_slice(wire), &limit).err(),
        Some(DecodeError::MessageTooLarge)
    );
    let retained = FieldOptionsOwnedView::decode(Bytes::copy_from_slice(wire)).expect("original field options");
    assert_eq!(retained.into_bytes().as_ref(), wire);
}

#[test]
fn malformed_descriptor_annotations_are_rejected_before_retaining_bytes() {
    assert_malformed::<MessageOptions>(b"\x0a\x02\x20", DecodeError::UnexpectedEof);
    assert_malformed::<Template>(b"\x0a\x01\xff", DecodeError::InvalidUtf8);
    assert_malformed::<HelpLink>(b"\x12\x01\xff", DecodeError::InvalidUtf8);
    assert_malformed::<MetadataEntry>(b"\x0a\x01\xff", DecodeError::InvalidUtf8);
    assert_malformed::<FieldOptions>(b"\x12\x01\xff", DecodeError::InvalidUtf8);
    assert_malformed::<FieldOptions>(b"\x1a\x01\xff", DecodeError::InvalidUtf8);
    assert_malformed::<FileOptions>(b"\x0a\x01\xff", DecodeError::InvalidUtf8);
}

#[cfg(feature = "decider")]
#[test]
fn public_decider_registry_retains_the_schema_fault_type_names() {
    let mut registry = TypeRegistry::new();
    crate::decider::v1::register_types(&mut registry);
    for name in [
        "CommandTypeUnroutableError",
        "CommandRequestMalformedError",
        "ExpectedRevisionUnsatisfiableError",
        "StreamWriteConflictError",
        "GuestFaultError",
        "GuestDeadlineExceededError",
        "StorageUnavailableError",
        "HostInternalError",
        "AdmissionLimitReachedError",
        "PrincipalMissingError",
        "PrincipalUnauthorizedError",
    ] {
        let type_url = format!("type.googleapis.com/trogonai.decider.v1.{name}");
        let entry = registry.json_any_by_url(&type_url).expect("schema marker registration");
        assert_eq!(entry.type_url, type_url);
        assert!(!entry.is_wkt);
    }
    assert!(
        registry
            .json_any_by_url("type.googleapis.com/trogonai.decider.v1.GUEST_FAULT")
            .is_none()
    );
}
