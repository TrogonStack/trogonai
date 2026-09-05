use std::fmt::Debug;

use buffa::bytes::Bytes;
use buffa::json_helpers::ProtoElemJson;
use buffa::{DecodeError, DecodeOptions, HasMessageView, Message, MessageView, ViewEncode};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[cfg(feature = "schedules")]
mod datetime_tests;
mod error_options_tests;
mod error_retained_tests;
mod retained_fixture;
#[cfg(feature = "grpc-nats-micro")]
mod transport_retained_tests;
mod wire_boundary_tests;

#[cfg(all(feature = "decider", feature = "schedules"))]
mod decider_retained_tests;
#[cfg(all(feature = "decider", feature = "schedules"))]
mod decider_tests;
#[cfg(feature = "decider")]
mod fault_codec_tests;
#[cfg(any(feature = "decider", feature = "grpc-nats-micro"))]
mod google_rpc_tests;
#[cfg(any(feature = "decider", feature = "grpc-nats-micro"))]
mod map_boundary_tests;
#[cfg(feature = "grpc-nats-micro")]
mod protocol_tests;
#[cfg(feature = "schedules")]
mod retained_view_tests;
#[cfg(any(feature = "decider", feature = "grpc-nats-micro"))]
mod rpc_retained_tests;
#[cfg(feature = "schedules")]
mod scheduler_json_tests;
#[cfg(feature = "schedules")]
mod scheduler_presence_tests;
#[cfg(feature = "schedules")]
mod scheduler_retained_tests;
#[cfg(feature = "schedules")]
mod scheduler_tests;

fn assert_wire_codec<M>(wire: &[u8], expected: &M)
where
    M: Message + HasMessageView + Debug + PartialEq,
    for<'a> M::View<'a>: ViewEncode<'a>,
{
    assert_eq!(&M::decode_from_slice(wire).expect("owned decode"), expected);

    let view = M::decode_view(wire).expect("borrowed decode");
    assert_eq!(&view.to_owned_message().expect("view conversion"), expected);
    assert_eq!(
        &M::decode_from_slice(&view.try_encode_to_vec().expect("view encode")).expect("view wire decode"),
        expected
    );

    let options = DecodeOptions::new().with_max_message_size(wire.len());
    let view = M::decode_view_with_options(wire, &options).expect("bounded borrowed decode");
    assert_eq!(&view.to_owned_message().expect("view conversion"), expected);
    let handle =
        M::decode_view_handle_with_options(Bytes::copy_from_slice(wire), &options).expect("bounded retained decode");
    assert_eq!(&handle.as_ref().to_owned_message(), expected);

    let mut extended = wire.to_vec();
    extended.extend_from_slice(&[0xf8, 0x07, 0x01]);
    assert_eq!(
        &M::decode_from_slice(&extended).expect("unknown field owned decode"),
        expected
    );
    let handle = M::decode_view_handle(Bytes::from(extended)).expect("unknown field retained decode");
    assert_eq!(&handle.as_ref().to_owned_message(), expected);
}

fn assert_json_codec<M>(json: Value) -> M
where
    M: Message + HasMessageView + Debug + PartialEq + Serialize + DeserializeOwned + ProtoElemJson,
    for<'a> M::View<'a>: ViewEncode<'a> + Serialize,
    M::ViewHandle: Serialize,
{
    let message: M = serde_json::from_value(json.clone()).expect("JSON fixture decode");
    assert_eq!(serde_json::to_value(&message).expect("owned JSON"), json);
    assert_proto_sequence(vec![message.clone()], json!([json.clone()]));
    let wire = message.try_encode_to_vec().expect("owned encode");
    assert_wire_codec(&wire, &message);
    let view = M::decode_view(&wire).expect("borrowed decode");
    assert_eq!(serde_json::to_value(&view).expect("borrowed JSON"), json);
    drop(view);
    let handle = M::decode_view_handle(Bytes::from(wire)).expect("retained decode");
    assert_eq!(serde_json::to_value(&handle).expect("retained JSON"), json);

    let empty: M = serde_json::from_value(json!({})).expect("absent JSON fields");
    assert_wire_codec(&[], &empty);
    let mut reused = message.clone();
    reused.clear();
    assert_eq!(reused, empty);
    assert_wire_codec(&reused.try_encode_to_vec().expect("cleared wire"), &empty);
    assert_eq!(
        serde_json::to_value(&reused).expect("cleared JSON"),
        serde_json::to_value(&empty).expect("default JSON")
    );
    message
}

#[derive(Serialize, Deserialize)]
#[serde(bound = "M: ProtoElemJson")]
struct ProtoBatch<M> {
    #[serde(with = "buffa::json_helpers::proto_seq")]
    messages: Vec<M>,
}

fn assert_proto_sequence<T: ProtoElemJson + Debug + PartialEq>(values: Vec<T>, expected: Value) {
    let batch = ProtoBatch { messages: values };
    let batch_json = json!({"messages": expected});
    assert_eq!(serde_json::to_value(&batch).expect("repeated field JSON"), batch_json);
    let decoded: ProtoBatch<T> = serde_json::from_value(batch_json).expect("repeated field decode");
    assert_eq!(decoded.messages, batch.messages);
    assert!(serde_json::from_value::<ProtoBatch<T>>(json!({"messages": [null]})).is_err());
    let empty: ProtoBatch<T> = serde_json::from_value(json!({"messages": null})).expect("null collection");
    assert!(empty.messages.is_empty());
}

fn assert_malformed<M: Message + HasMessageView>(wire: &[u8], expected: DecodeError) {
    assert_eq!(M::decode_from_slice(wire).err(), Some(expected.clone()));
    assert_eq!(M::decode_view(wire).err(), Some(expected.clone()));
    assert_eq!(
        M::decode_view_handle(Bytes::copy_from_slice(wire)).err(),
        Some(expected)
    );
}
