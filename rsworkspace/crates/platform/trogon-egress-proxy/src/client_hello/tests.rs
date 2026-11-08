use super::*;

fn u16_be(value: usize) -> [u8; 2] {
    u16::try_from(value).expect("fits u16").to_be_bytes()
}

fn extension(kind: u16, data: &[u8]) -> Vec<u8> {
    let mut out = kind.to_be_bytes().to_vec();
    out.extend(u16_be(data.len()));
    out.extend(data);
    out
}

fn sni_entry(kind: u8, name: &[u8]) -> Vec<u8> {
    let mut out = vec![kind];
    out.extend(u16_be(name.len()));
    out.extend(name);
    out
}

fn sni_extension(entries: &[Vec<u8>]) -> Vec<u8> {
    let list: Vec<u8> = entries.concat();
    let mut data = u16_be(list.len()).to_vec();
    data.extend(list);
    extension(TLS_EXTENSION_SERVER_NAME, &data)
}

fn hello_body(extensions: Option<&[u8]>) -> Vec<u8> {
    let mut body = vec![3, 3];
    body.extend([7u8; 32]);
    body.push(4);
    body.extend([9u8; 4]);
    body.extend(u16_be(4));
    body.extend([0x13, 0x01, 0x13, 0x02]);
    body.extend([1, 0]);
    if let Some(extensions) = extensions {
        body.extend(u16_be(extensions.len()));
        body.extend(extensions);
    }
    body
}

fn record_from_hello(hello: &[u8]) -> Vec<u8> {
    let mut handshake = vec![TLS_HANDSHAKE_CLIENT_HELLO];
    handshake.extend(&u32::try_from(hello.len()).expect("fits").to_be_bytes()[1..]);
    handshake.extend(hello);
    record_from_handshake(&handshake)
}

fn record_from_handshake(handshake: &[u8]) -> Vec<u8> {
    let mut record = vec![TLS_CONTENT_TYPE_HANDSHAKE, 3, 1];
    record.extend(u16_be(handshake.len()));
    record.extend(handshake);
    record
}

fn record_with_extensions(extensions: &[u8]) -> Vec<u8> {
    record_from_hello(&hello_body(Some(extensions)))
}

fn record_for(name: &str) -> Vec<u8> {
    record_with_extensions(&sni_extension(&[sni_entry(0, name.as_bytes())]))
}

#[test]
fn reads_the_single_host_name() {
    assert_eq!(
        server_name(&record_for("api.example.com")).expect("sni").as_str(),
        "api.example.com"
    );
}

#[test]
fn folds_the_name_to_lowercase() {
    assert_eq!(
        server_name(&record_for("API.Example.COM")).expect("sni").as_str(),
        "api.example.com"
    );
}

#[test]
fn finds_the_extension_among_others_in_any_position() {
    let mut extensions = extension(0x002b, &[2, 3, 4]);
    extensions.extend(extension(0x000a, &[0, 2, 0, 0x1d]));
    extensions.extend(sni_extension(&[sni_entry(0, b"example.org")]));
    extensions.extend(extension(0x0010, &[0, 3, 2, b'h', b'2']));
    assert_eq!(
        server_name(&record_with_extensions(&extensions)).expect("sni").as_str(),
        "example.org"
    );
}

#[test]
fn ignores_bytes_after_the_client_hello_inside_the_record() {
    let hello = hello_body(Some(&sni_extension(&[sni_entry(0, b"example.org")])));
    let mut handshake = vec![TLS_HANDSHAKE_CLIENT_HELLO];
    handshake.extend(&u32::try_from(hello.len()).expect("fits").to_be_bytes()[1..]);
    handshake.extend(&hello);
    handshake.extend([0x0e, 0, 0, 0]);
    assert_eq!(
        server_name(&record_from_handshake(&handshake)).expect("sni").as_str(),
        "example.org"
    );
}

#[test]
fn ignores_bytes_after_the_record() {
    let mut wire = record_for("example.org");
    wire.extend(b"application bytes that follow");
    assert_eq!(server_name(&wire).expect("sni").as_str(), "example.org");
}

#[test]
fn no_extensions_block_means_no_server_name() {
    assert_eq!(
        server_name(&record_from_hello(&hello_body(None))),
        Err(ClientHelloError::NoServerName)
    );
}

#[test]
fn extensions_without_server_name_means_no_server_name() {
    assert_eq!(
        server_name(&record_with_extensions(&extension(0x002b, &[2, 3, 4]))),
        Err(ClientHelloError::NoServerName)
    );
    assert_eq!(
        server_name(&record_with_extensions(&[])),
        Err(ClientHelloError::NoServerName)
    );
}

#[test]
fn two_names_in_one_list_are_refused() {
    let extensions = sni_extension(&[sni_entry(0, b"a.example.com"), sni_entry(0, b"b.example.com")]);
    assert_eq!(
        server_name(&record_with_extensions(&extensions)),
        Err(ClientHelloError::MultipleServerNames)
    );
    let same = sni_extension(&[sni_entry(0, b"a.example.com"), sni_entry(0, b"a.example.com")]);
    assert_eq!(
        server_name(&record_with_extensions(&same)),
        Err(ClientHelloError::MultipleServerNames)
    );
}

#[test]
fn a_repeated_server_name_extension_is_refused() {
    let mut extensions = sni_extension(&[sni_entry(0, b"a.example.com")]);
    extensions.extend(sni_extension(&[sni_entry(0, b"b.example.com")]));
    assert_eq!(
        server_name(&record_with_extensions(&extensions)),
        Err(ClientHelloError::Malformed)
    );
}

#[test]
fn empty_and_non_host_name_lists_are_refused() {
    assert_eq!(
        server_name(&record_with_extensions(&sni_extension(&[]))),
        Err(ClientHelloError::Malformed)
    );
    assert_eq!(
        server_name(&record_with_extensions(&sni_extension(&[sni_entry(1, b"example.com")]))),
        Err(ClientHelloError::Malformed)
    );
    assert_eq!(
        server_name(&record_with_extensions(&sni_extension(&[
            sni_entry(1, b"x"),
            sni_entry(0, b"example.com")
        ]))),
        Err(ClientHelloError::Malformed)
    );
}

#[test]
fn names_that_are_not_dns_host_names_are_refused() {
    for name in [
        "",
        "a b.example",
        "exa/mple.com",
        "example.com.",
        "127.0.0.1",
        "*.example.com",
        "bücher.example",
    ] {
        let record = record_with_extensions(&sni_extension(&[sni_entry(0, name.as_bytes())]));
        assert!(
            matches!(server_name(&record), Err(ClientHelloError::InvalidServerName(_))),
            "{name:?} must be refused, got {:?}",
            server_name(&record)
        );
    }
    let record = record_with_extensions(&sni_extension(&[sni_entry(0, &[0xff, 0xfe])]));
    assert_eq!(server_name(&record), Err(ClientHelloError::Malformed));
}

#[test]
fn a_server_name_list_length_that_disagrees_with_the_extension_is_refused() {
    let mut data = u16_be(100).to_vec();
    data.extend(sni_entry(0, b"example.com"));
    assert_eq!(
        server_name(&record_with_extensions(&extension(TLS_EXTENSION_SERVER_NAME, &data))),
        Err(ClientHelloError::Malformed)
    );
    let mut data = u16_be(2).to_vec();
    data.extend(sni_entry(0, b"example.com"));
    assert_eq!(
        server_name(&record_with_extensions(&extension(TLS_EXTENSION_SERVER_NAME, &data))),
        Err(ClientHelloError::Malformed)
    );
}

#[test]
fn non_handshake_first_records_are_refused() {
    assert_eq!(
        server_name(b"GET / HTTP/1.1\r\n"),
        Err(ClientHelloError::NotTlsHandshake)
    );
    assert_eq!(
        server_name(&[0x17, 3, 3, 0, 1, 0]),
        Err(ClientHelloError::NotTlsHandshake)
    );
    assert_eq!(
        server_name(&[0x16, 2, 0, 0, 1, 0]),
        Err(ClientHelloError::NotTlsHandshake)
    );
    assert_eq!(
        server_name(&[0x16, 3, 0, 0, 1, 0]),
        Err(ClientHelloError::NotTlsHandshake)
    );
    assert_eq!(
        server_name(&[0x16, 3, 9, 0, 1, 0]),
        Err(ClientHelloError::NotTlsHandshake)
    );
    assert_eq!(
        server_name(&[0x80, 0x2e, 1, 0, 2]),
        Err(ClientHelloError::NotTlsHandshake)
    );
}

#[test]
fn non_client_hello_handshakes_are_refused() {
    let mut handshake = vec![2u8, 0, 0, 0];
    handshake.extend([0u8; 4]);
    assert_eq!(
        server_name(&record_from_handshake(&handshake)),
        Err(ClientHelloError::NotClientHello)
    );
}

#[test]
fn bad_record_lengths_are_refused() {
    assert_eq!(server_name(&[0x16, 3, 1, 0, 0]), Err(ClientHelloError::BadRecordLength));
    assert_eq!(
        server_name(&[0x16, 3, 1, 0x40, 0x01]),
        Err(ClientHelloError::BadRecordLength)
    );
    assert_eq!(
        server_name(&[0x16, 3, 1, 0xff, 0xff]),
        Err(ClientHelloError::BadRecordLength)
    );
}

#[test]
fn a_client_hello_spanning_records_is_refused() {
    let hello = hello_body(Some(&sni_extension(&[sni_entry(0, b"example.com")])));
    let mut handshake = vec![TLS_HANDSHAKE_CLIENT_HELLO];
    handshake.extend(&u32::try_from(hello.len() + 50).expect("fits").to_be_bytes()[1..]);
    handshake.extend(&hello);
    assert_eq!(
        server_name(&record_from_handshake(&handshake)),
        Err(ClientHelloError::Fragmented)
    );
}

#[test]
fn every_proper_prefix_is_incomplete_with_a_growing_requirement() {
    let record = record_for("api.example.com");
    for len in 0..record.len() {
        match server_name(&record[..len]) {
            Err(ClientHelloError::Incomplete { needed }) => {
                assert!(needed > len, "prefix {len} claims to need only {needed}");
                assert!(needed <= record.len());
            }
            other => panic!("prefix of {len} bytes should be incomplete, got {other:?}"),
        }
    }
    assert!(server_name(&record).is_ok());
}

#[test]
fn a_prefix_of_a_non_tls_stream_fails_without_waiting() {
    assert_eq!(server_name(b"G"), Err(ClientHelloError::NotTlsHandshake));
    assert_eq!(server_name(&[0x16, 0x04]), Err(ClientHelloError::NotTlsHandshake));
}

#[test]
fn a_record_cut_short_inside_the_hello_is_malformed_not_a_panic() {
    let hello = hello_body(Some(&sni_extension(&[sni_entry(0, b"example.com")])));
    for cut in 0..hello.len() {
        let mut handshake = vec![TLS_HANDSHAKE_CLIENT_HELLO];
        handshake.extend(&u32::try_from(cut).expect("fits").to_be_bytes()[1..]);
        handshake.extend(&hello[..cut]);
        let result = server_name(&record_from_handshake(&handshake));
        assert!(
            matches!(
                result,
                Err(ClientHelloError::Malformed | ClientHelloError::NoServerName)
            ),
            "cut at {cut}: {result:?}"
        );
    }
}

#[test]
fn inconsistent_inner_lengths_are_malformed() {
    let mut extensions_len_too_long = hello_body(Some(&sni_extension(&[sni_entry(0, b"example.com")])));
    // legacy_version, random, session id (1 + 4), cipher suites (2 + 4), compression (2)
    let extensions_len_at = 2 + 32 + 5 + 6 + 2;
    extensions_len_too_long[extensions_len_at + 1] += 1;
    assert_eq!(
        server_name(&record_from_hello(&extensions_len_too_long)),
        Err(ClientHelloError::Malformed)
    );

    let mut odd_cipher_suites = hello_body(Some(&sni_extension(&[sni_entry(0, b"example.com")])));
    odd_cipher_suites[2 + 32 + 1 + 4 + 1] = 3;
    assert_eq!(
        server_name(&record_from_hello(&odd_cipher_suites)),
        Err(ClientHelloError::Malformed)
    );

    let mut oversized_session_id = hello_body(None);
    oversized_session_id[2 + 32] = 200;
    assert_eq!(
        server_name(&record_from_hello(&oversized_session_id)),
        Err(ClientHelloError::Malformed)
    );
}

#[test]
fn arbitrary_byte_patterns_never_panic() {
    let seeds: [&[u8]; 6] = [
        &[0x16, 3, 1, 0, 4, 1, 0, 0, 0],
        &[0x16, 3, 3, 0, 5, 1, 0, 0, 1, 0],
        &[0x16, 3, 1, 0, 1, 1],
        &[0x16, 3, 1, 0, 3, 1, 0xff, 0xff],
        &[0x16, 3, 1, 0x3f, 0xff],
        &[0xff; 64],
    ];
    for seed in seeds {
        let _ = server_name(seed);
    }
    let record = record_for("example.com");
    for index in 0..record.len() {
        for value in [0x00, 0x01, 0x7f, 0xff] {
            let mut mutated = record.clone();
            mutated[index] = value;
            let _ = server_name(&mutated);
        }
    }
}
