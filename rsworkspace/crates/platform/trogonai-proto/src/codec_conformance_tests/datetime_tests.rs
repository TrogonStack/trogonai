use buffa::bytes::Bytes;
use buffa::{DecodeError, DecodeOptions, HasMessageView, Message, OwnedView};
use serde_json::json;

use super::retained_fixture::retained_detail;
use super::{assert_json_codec, assert_wire_codec};
use crate::google::r#type::{DateTime, DateTimeOwnedView, DateTimeView, TimeZone, TimeZoneOwnedView, TimeZoneView};

#[test]
fn civil_datetime_preserves_all_coordinates_through_retained_transfer() {
    for offset in [
        json!({"utcOffset": "-18000s"}),
        json!({"timeZone": {"id": "America/New_York", "version": "2025b"}}),
    ] {
        let mut expected =
            json!({"year": 2026, "month": 9, "day": 5, "hours": 11, "minutes": 12, "seconds": 13, "nanos": 123456789});
        expected
            .as_object_mut()
            .expect("datetime object")
            .extend(offset.as_object().expect("offset object").clone());
        retained_detail!(DateTime, DateTimeOwnedView, DateTimeView<'static>, expected, |handle| {
            assert_eq!(handle.year(), 2026);
            assert_eq!(handle.month(), 9);
            assert_eq!(handle.day(), 5);
            assert_eq!(handle.hours(), 11);
            assert_eq!(handle.minutes(), 12);
            assert_eq!(handle.seconds(), 13);
            assert_eq!(handle.nanos(), 123_456_789);
            assert!(handle.time_offset().is_some());
        });
    }
    retained_detail!(
        TimeZone,
        TimeZoneOwnedView,
        TimeZoneView<'static>,
        json!({"id": "Europe/Paris", "version": "2025b"}),
        |handle| {
            assert_eq!(handle.id(), "Europe/Paris");
            assert_eq!(handle.version(), "2025b");
        }
    );
}

#[test]
fn datetime_offset_json_rejects_conflicts_and_normalizes_field_aliases() {
    for (input, expected) in [
        (json!({"utc_offset": "0s"}), json!({"utcOffset": "0s"})),
        (json!({"time_zone": {"id": "UTC"}}), json!({"timeZone": {"id": "UTC"}})),
        (
            json!({"utcOffset": null, "timeZone": {"id": "UTC"}}),
            json!({"timeZone": {"id": "UTC"}}),
        ),
        (json!({"timeZone": null, "utcOffset": "0s"}), json!({"utcOffset": "0s"})),
    ] {
        let value: DateTime = serde_json::from_value(input).expect("datetime input");
        assert_eq!(serde_json::to_value(value).expect("normalized datetime"), expected);
    }
    for input in [
        r#"{"utcOffset":"0s","utc_offset":"1s"}"#,
        r#"{"utcOffset":"0s","timeZone":{"id":"UTC"}}"#,
    ] {
        assert!(serde_json::from_str::<DateTime>(input).is_err());
    }
    let repeated: DateTime = serde_json::from_str(r#"{"year":2026,"year":2027}"#).expect("repeated scalar");
    assert_eq!(repeated.year, 2027);
}

#[test]
fn repeated_timezone_fragments_merge_until_an_offset_replaces_them() {
    let timezone = b"\x4a\x05\x0a\x03UTC\x4a\x07\x12\x052025b";
    let expected = assert_json_codec::<DateTime>(json!({"timeZone": {"id": "UTC", "version": "2025b"}}));
    assert_wire_codec(timezone, &expected);
    let expected = assert_json_codec::<DateTime>(json!({"utcOffset": "1s"}));
    assert_wire_codec(&[timezone.as_slice(), b"\x42\x02\x08\x01"].concat(), &expected);
    let view = DateTime::decode_view(b"").expect("civil datetime without offset");
    assert!(view.time_offset.is_none());
}
