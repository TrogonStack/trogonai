use buffa::bytes::Bytes;
use buffa::{DecodeError, DecodeOptions, Message, OwnedView};
use serde_json::json;

use super::assert_json_codec;
use super::retained_fixture::retained_detail;
use crate::google::rpc::Code;
use crate::grpc_nats_micro::v1::{
    FailRequest, FailRequestOwnedView, FailRequestView, FailResponse, FailResponseOwnedView, FailResponseView,
    SayRequest, SayRequestOwnedView, SayRequestView, SayResponse, SayResponseOwnedView, SayResponseView,
};
use crate::nats::micro::v1alpha1::{
    ContentType, MethodOptions, MethodOptionsOwnedView, MethodOptionsView, ServiceOptions, ServiceOptionsOwnedView,
    ServiceOptionsView,
};

#[test]
fn retained_rpc_messages_preserve_optional_empty_and_absent_fields() {
    for message in [None, Some(""), Some("hello")] {
        let expected = message.map_or_else(|| json!({}), |message| json!({"message": message}));
        retained_detail!(
            SayRequest,
            SayRequestOwnedView,
            SayRequestView<'static>,
            expected.clone(),
            |handle| {
                assert_eq!(handle.message(), message);
            }
        );
        retained_detail!(
            SayResponse,
            SayResponseOwnedView,
            SayResponseView<'static>,
            expected.clone(),
            |handle| {
                assert_eq!(handle.message(), message);
            }
        );
        retained_detail!(
            FailResponse,
            FailResponseOwnedView,
            FailResponseView<'static>,
            expected,
            |handle| {
                assert_eq!(handle.message(), message);
            }
        );
    }
}

#[test]
fn retained_rpc_failure_preserves_explicit_success_and_fault_codes() {
    retained_detail!(
        FailRequest,
        FailRequestOwnedView,
        FailRequestView<'static>,
        json!({
            "code": "UNAVAILABLE", "message": "retry later"
        }),
        |handle| {
            assert_eq!(handle.code(), Some(Code::UNAVAILABLE.into()));
            assert_eq!(handle.message(), Some("retry later"));
        }
    );
    retained_detail!(
        FailRequest,
        FailRequestOwnedView,
        FailRequestView<'static>,
        json!({"code": "OK"}),
        |handle| {
            assert_eq!(handle.code(), Some(Code::OK.into()));
            assert_eq!(handle.message(), None);
        }
    );
    retained_detail!(
        FailRequest,
        FailRequestOwnedView,
        FailRequestView<'static>,
        json!({}),
        |handle| {
            assert_eq!(handle.code(), None);
            assert_eq!(handle.message(), None);
        }
    );
}

#[test]
fn retained_discovery_metadata_survives_service_configuration_reload() {
    retained_detail!(
        ServiceOptions,
        ServiceOptionsOwnedView,
        ServiceOptionsView<'static>,
        json!({
            "version": "1.2.3", "description": "Scheduling", "metadata": {"region": "west"},
            "contentType": "CONTENT_TYPE_JSON"
        }),
        |handle| {
            assert_eq!(handle.version(), Some("1.2.3"));
            assert_eq!(handle.description(), Some("Scheduling"));
            assert_eq!(handle.metadata().get("region"), Some(&"west"));
            assert_eq!(handle.content_type(), ContentType::Json);
        }
    );
    retained_detail!(
        MethodOptions,
        MethodOptionsOwnedView,
        MethodOptionsView<'static>,
        json!({
            "metadata": {"region": "west", "owner": "scheduler"}
        }),
        |handle| {
            assert_eq!(handle.metadata().get("region"), Some(&"west"));
            assert_eq!(handle.metadata().get("owner"), Some(&"scheduler"));
        }
    );
}
