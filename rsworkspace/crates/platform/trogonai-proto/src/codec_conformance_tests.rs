use std::fmt::Debug;

use buffa::bytes::Bytes;
use buffa::{DecodeError, DecodeOptions, HasMessageView, Message, MessageView, ViewEncode};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

#[cfg(all(feature = "decider", feature = "schedules"))]
mod decider_tests;
#[cfg(any(feature = "decider", feature = "grpc-nats-micro"))]
mod google_rpc_tests;
#[cfg(feature = "grpc-nats-micro")]
mod protocol_tests;
#[cfg(feature = "schedules")]
mod retained_view_tests;
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
    M: Message + HasMessageView + Debug + PartialEq + Serialize + DeserializeOwned,
    for<'a> M::View<'a>: ViewEncode<'a> + Serialize,
    M::ViewHandle: Serialize,
{
    let message: M = serde_json::from_value(json.clone()).expect("JSON fixture decode");
    assert_eq!(serde_json::to_value(&message).expect("owned JSON"), json);
    let wire = message.try_encode_to_vec().expect("owned encode");
    assert_wire_codec(&wire, &message);
    let view = M::decode_view(&wire).expect("borrowed decode");
    assert_eq!(serde_json::to_value(&view).expect("borrowed JSON"), json);
    drop(view);
    let handle = M::decode_view_handle(Bytes::from(wire)).expect("retained decode");
    assert_eq!(serde_json::to_value(&handle).expect("retained JSON"), json);
    message
}

fn assert_malformed<M: Message + HasMessageView>(wire: &[u8], expected: DecodeError) {
    assert_eq!(M::decode_from_slice(wire).err(), Some(expected.clone()));
    assert_eq!(M::decode_view(wire).err(), Some(expected.clone()));
    assert_eq!(
        M::decode_view_handle(Bytes::copy_from_slice(wire)).err(),
        Some(expected)
    );
}
