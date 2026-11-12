use std::fmt::Debug;

use buffa::bytes::Bytes;
use buffa::{DecodeError, DecodeOptions, HasMessageView, Message, MessageView, ViewEncode};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

mod boundaries_tests;
mod builders_tests;
mod events_tests;
mod fixtures;
mod messages_tests;
mod public_apis_tests;
mod reference_fields_tests;
mod rpc_tests;
mod writer_failures;

fn assert_wire_codec<M>(wire: &[u8], expected: &M)
where
    M: Message + HasMessageView + Debug + PartialEq,
    for<'a> M::View<'a>: ViewEncode<'a>,
{
    assert_eq!(&M::decode_from_slice(wire).expect("owned wire decode"), expected);
    let view = M::decode_view(wire).expect("borrowed wire decode");
    assert_eq!(
        &view.to_owned_message().expect("borrowed ownership conversion"),
        expected
    );
    let encoded = view.try_encode_to_vec().expect("borrowed wire encode");
    assert_eq!(
        &M::decode_from_slice(&encoded).expect("borrowed wire roundtrip"),
        expected
    );
    let limit = DecodeOptions::new().with_max_message_size(wire.len());
    let bounded = M::decode_view_with_options(wire, &limit).expect("bounded borrowed decode");
    assert_eq!(&bounded.to_owned_message().expect("bounded owned conversion"), expected);
    let retained =
        M::decode_view_handle_with_options(Bytes::copy_from_slice(wire), &limit).expect("bounded retained decode");
    assert_eq!(&retained.as_ref().to_owned_message(), expected);
    let mut extended = wire.to_vec();
    extended.extend_from_slice(&[0xf8, 0x07, 0x01]);
    let unknown = M::decode_from_slice(&extended).expect("owned decode skips unknown wire fields");
    assert_eq!(&unknown, expected);
    assert_eq!(unknown.encode_to_vec(), expected.encode_to_vec());
}

fn assert_json_codec<M>(fixture: Value) -> M
where
    M: Message + HasMessageView + Debug + PartialEq + Serialize + DeserializeOwned,
    for<'a> M::View<'a>: ViewEncode<'a> + Serialize,
    M::ViewHandle: Serialize,
{
    let message: M = serde_json::from_value(fixture.clone()).expect("peer JSON fixture");
    assert_eq!(serde_json::to_value(&message).expect("owned JSON"), fixture);
    let wire = message.try_encode_to_vec().expect("owned binary encoding");
    assert_wire_codec(&wire, &message);
    let view = M::decode_view(&wire).expect("borrowed decode");
    assert_eq!(serde_json::to_value(&view).expect("borrowed JSON"), fixture);
    drop(view);
    let retained = M::decode_view_handle(Bytes::from(wire)).expect("retained decode");
    assert_eq!(serde_json::to_value(&retained).expect("retained JSON"), fixture);

    let absent: M = serde_json::from_value(json!({})).expect("absent peer fields");
    assert_wire_codec(&[], &absent);
    let absent_view = M::decode_view(&[]).expect("absent borrowed fields");
    assert_eq!(
        serde_json::to_value(&absent_view).expect("absent borrowed JSON"),
        json!({})
    );
    let mut reused = message.clone();
    reused.clear();
    assert_eq!(reused, absent);
    assert_eq!(serde_json::to_value(&reused).expect("cleared JSON"), json!({}));
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

macro_rules! retained_message {
    ($owned:ty, $retained:ty, $fixture:expr, |$handle:ident| $checks:block) => {{
        let fixture = $fixture;
        let mut source = assert_json_codec::<$owned>(fixture.clone());
        let expected = source.clone();
        let retained = <$retained>::from_owned(&source).expect("retain peer message");
        source.clear();
        let independent = retained.clone();
        drop(retained);
        let $handle = std::thread::spawn(move || independent).join().expect("transfer peer view");
        $checks
        assert_eq!($handle.to_owned_message(), expected);
        assert_eq!(serde_json::to_value($handle.view()).expect("retained view JSON"), fixture);

        let canonical = expected.try_encode_to_vec().expect("canonical known fields");
        let mut extended = $handle.into_bytes().to_vec();
        extended.extend_from_slice(&[0xf8, 0x07, 0x01]);
        let original = buffa::bytes::Bytes::from(extended);
        let too_small = buffa::DecodeOptions::new().with_max_message_size(original.len() - 1);
        assert_eq!(
            <$retained>::decode_with_options(original.clone(), &too_small).err(),
            Some(buffa::DecodeError::MessageTooLarge)
        );
        let limit = buffa::DecodeOptions::new().with_max_message_size(original.len());
        let retained = <$retained>::decode_with_options(original.clone(), &limit).expect("exact size bound");
        assert_eq!(retained.bytes().as_ptr(), original.as_ptr());
        assert_eq!(retained.to_owned_message().try_encode_to_vec().expect("owned known fields"), canonical);
        let raw: buffa::OwnedView<<$owned as buffa::HasMessageView>::View<'static>> = retained.into();
        let $handle = <$retained>::from(raw);
        $checks
        let transferred = $handle.into_bytes();
        assert_eq!(transferred, original, "raw retained transfer preserves unknown wire fields");
        let decoded = <$retained>::decode(transferred).expect("transferred message");
        assert_eq!(decoded.to_owned_message(), expected);
    }};
}

use retained_message;
