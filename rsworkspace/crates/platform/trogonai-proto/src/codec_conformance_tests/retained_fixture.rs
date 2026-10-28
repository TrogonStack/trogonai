macro_rules! retained_detail {
    ($owned:ty, $retained:ty, $view:ty, $json:expr, |$handle:ident| $inspect:block) => {{
        let expected = $json;
        let mut source = assert_json_codec::<$owned>(expected.clone());
        let retained = <$retained>::from_owned(&source).expect("retain owned detail");
        source.clear();
        let $handle = &retained;
        $inspect
        assert_eq!(serde_json::to_value(retained.view()).expect("retained JSON"), expected);

        let mut wire = retained.into_bytes().to_vec();
        wire.extend_from_slice(b"\xf8\x07\x01");
        let original = Bytes::from(wire);
        let limit = DecodeOptions::new().with_max_message_size(original.len() - 1);
        assert_eq!(
            <$retained>::decode_with_options(original.clone(), &limit).err(),
            Some(DecodeError::MessageTooLarge)
        );
        let limit = DecodeOptions::new().with_max_message_size(original.len());
        let retained = <$retained>::decode_with_options(original.clone(), &limit).expect("bounded detail");
        assert_eq!(retained.bytes(), &original);
        assert_eq!(retained.bytes().as_ptr(), original.as_ptr());
        let clone = retained.clone();
        drop(retained);
        let raw: OwnedView<$view> = clone.into();
        let retained = <$retained>::from(raw);
        let $handle = &retained;
        $inspect
        let transferred = std::thread::spawn(move || retained.into_bytes()).join().expect("transfer thread");
        assert_eq!(transferred, original);
        let retained = <$retained>::decode(transferred).expect("transferred detail");
        assert_eq!(serde_json::to_value(retained.to_owned_message()).expect("owned detail JSON"), expected);
    }};
}

pub(super) use retained_detail;
