use super::*;
    use wasmtime::Module;

    #[test]
    fn builds_default_compatible_engine() {
        let engine = new_engine().expect("wasmtime engine");
        drop(engine);
    }

    #[test]
    fn identity_guest_round_trips_utf8_payload() {
        let engine = new_engine().unwrap();
        let wasm = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/identity_redact_part.wasm"
        ));
        let module = Module::from_binary(&engine, wasm).unwrap();
        let inp = br#"{"k":42}"#.to_vec();
        let got = redact_part_guest(&engine, &module, &inp).unwrap();
        assert_eq!(got, inp);
    }
