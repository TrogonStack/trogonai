use buffa::bytes::Bytes;
use buffa::{DecodeError, DecodeOptions, Message, OwnedView};
use serde_json::{Value, json};

use super::assert_json_codec;
use super::retained_fixture::retained_detail;
use crate::r#gen::trogon::error::v1alpha1::field_options::ValuePolicyView;
use crate::r#gen::trogon::error::v1alpha1::message_options::{
    HelpLink, HelpLinkOwnedView, HelpLinkView, MetadataEntry, MetadataEntryOwnedView, MetadataEntryView, Template,
    TemplateOwnedView, TemplateView,
};
use crate::r#gen::trogon::error::v1alpha1::{
    Code, FieldOptions, FieldOptionsOwnedView, FieldOptionsView, MessageOptions, MessageOptionsOwnedView,
    MessageOptionsView, Visibility,
};

fn template() -> Value {
    json!({
        "domain": "scheduler.example", "reason": "CAPACITY_EXCEEDED", "message": "No worker slots available",
        "code": "RESOURCE_EXHAUSTED", "visibility": "VISIBILITY_PUBLIC",
        "helpLinks": [{"url": "https://example.com/capacity", "description": "Capacity limits"}],
        "metadata": [
            {"key": "worker", "value": "primary", "visibility": "VISIBILITY_PRIVATE"},
            {"key": "worker", "value": "standby", "visibility": "VISIBILITY_PRIVATE"}
        ]
    })
}

#[test]
fn template_ownership_preserves_error_identity_and_ordered_annotations() {
    retained_detail!(
        Template,
        TemplateOwnedView,
        TemplateView<'static>,
        template(),
        |handle| {
            assert_eq!(handle.domain(), "scheduler.example");
            assert_eq!(handle.reason(), "CAPACITY_EXCEEDED");
            assert_eq!(handle.message(), "No worker slots available");
            assert_eq!(handle.code(), Code::ResourceExhausted);
            assert_eq!(handle.visibility(), Visibility::Public);
            assert_eq!(handle.help_links()[0].url, "https://example.com/capacity");
            let metadata = &**handle.metadata();
            assert_eq!(metadata.len(), 2);
            assert_eq!(metadata[0].key, "worker");
            assert_eq!(metadata[0].value, "primary");
            assert_eq!(metadata[1].key, "worker");
            assert_eq!(metadata[1].value, "standby");
        }
    );
    retained_detail!(
        MessageOptions,
        MessageOptionsOwnedView,
        MessageOptionsView<'static>,
        json!({"template": template()}),
        |handle| {
            let annotation = handle.template().as_option().expect("present error template");
            assert_eq!(annotation.domain, "scheduler.example");
            assert_eq!(annotation.reason, "CAPACITY_EXCEEDED");
            assert_eq!(annotation.code, Code::ResourceExhausted);
            assert_eq!(serde_json::to_value(annotation).expect("template JSON"), template());
        }
    );
}

#[test]
fn help_and_metadata_ownership_retains_strings_and_exposure_policy() {
    retained_detail!(
        HelpLink,
        HelpLinkOwnedView,
        HelpLinkView<'static>,
        json!({"url": "https://example.com/capacity", "description": "Capacity limits"}),
        |handle| {
            assert_eq!(handle.url(), "https://example.com/capacity");
            assert_eq!(handle.description(), "Capacity limits");
        }
    );
    retained_detail!(
        MetadataEntry,
        MetadataEntryOwnedView,
        MetadataEntryView<'static>,
        json!({"key": "worker", "value": "primary", "visibility": "VISIBILITY_PRIVATE"}),
        |handle| {
            assert_eq!(handle.key(), "worker");
            assert_eq!(handle.value(), "primary");
            assert_eq!(handle.visibility(), Visibility::Private);
        }
    );
}

#[test]
fn field_policy_ownership_distinguishes_empty_fallback_from_fixed_value() {
    retained_detail!(
        FieldOptions,
        FieldOptionsOwnedView,
        FieldOptionsView<'static>,
        json!({"visibility": "VISIBILITY_PUBLIC", "defaultValue": ""}),
        |handle| {
            assert_eq!(handle.visibility(), Visibility::Public);
            assert!(matches!(handle.value_policy(), Some(ValuePolicyView::DefaultValue(""))));
        }
    );
    retained_detail!(
        FieldOptions,
        FieldOptionsOwnedView,
        FieldOptionsView<'static>,
        json!({"visibility": "VISIBILITY_PRIVATE", "value": "internal"}),
        |handle| {
            assert_eq!(handle.visibility(), Visibility::Private);
            assert!(matches!(
                handle.value_policy(),
                Some(ValuePolicyView::Value("internal"))
            ));
        }
    );
    retained_detail!(
        FieldOptions,
        FieldOptionsOwnedView,
        FieldOptionsView<'static>,
        json!({}),
        |handle| {
            assert_eq!(handle.visibility(), Visibility::Unspecified);
            assert!(handle.value_policy().is_none());
        }
    );
}
