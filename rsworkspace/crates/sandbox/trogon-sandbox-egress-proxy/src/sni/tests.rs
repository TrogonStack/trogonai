use std::io::Write as _;

use super::*;

fn u16_be(v: u16) -> [u8; 2] {
    v.to_be_bytes()
}

/// Builds a minimal but well-formed ClientHello TLS record carrying a
/// single SNI host name extension, so the parser can be exercised without
/// a real TLS stack.
fn build_client_hello_with_sni(hostname: &str) -> Vec<u8> {
    let mut server_name_list = Vec::new();
    server_name_list.push(SERVER_NAME_TYPE_HOST_NAME);
    server_name_list.extend_from_slice(&u16_be(hostname.len() as u16));
    server_name_list.extend_from_slice(hostname.as_bytes());

    let mut sni_extension_data = Vec::new();
    sni_extension_data.extend_from_slice(&u16_be(server_name_list.len() as u16));
    sni_extension_data.extend_from_slice(&server_name_list);

    let mut extensions = Vec::new();
    extensions.extend_from_slice(&u16_be(EXTENSION_TYPE_SERVER_NAME));
    extensions.extend_from_slice(&u16_be(sni_extension_data.len() as u16));
    extensions.extend_from_slice(&sni_extension_data);

    let mut body = Vec::new();
    body.extend_from_slice(&[0x03, 0x03]); // client_version: TLS 1.2
    body.extend_from_slice(&[0u8; 32]); // random
    body.push(0); // session_id len = 0
    body.extend_from_slice(&u16_be(2)); // cipher_suites len
    body.extend_from_slice(&[0x13, 0x01]); // one cipher suite
    body.push(1); // compression_methods len
    body.push(0); // null compression
    body.extend_from_slice(&u16_be(extensions.len() as u16));
    body.extend_from_slice(&extensions);

    let mut handshake = Vec::new();
    handshake.push(HANDSHAKE_TYPE_CLIENT_HELLO);
    let body_len = (body.len() as u32).to_be_bytes();
    handshake.extend_from_slice(&body_len[1..4]);
    handshake.extend_from_slice(&body);

    let mut record = Vec::new();
    record.push(CONTENT_TYPE_HANDSHAKE);
    record.extend_from_slice(&[0x03, 0x01]); // legacy record version
    record.extend_from_slice(&u16_be(handshake.len() as u16));
    record.extend_from_slice(&handshake);
    record
}

#[test]
fn a_client_hello_is_read_to_a_server_name() -> Result<(), Box<dyn std::error::Error>> {
    let name = |host: &str| ClientServerName::of_client_hello(&build_client_hello_with_sni(host));
    assert_eq!(
        name("Api.Example.com"),
        ClientServerName::Name("api.example.com".to_string())
    );
    assert_eq!(name("localhost"), ClientServerName::Name("localhost".to_string()));
    for unreadable in [
        "",
        "a b.example",
        "-a.example",
        "a-.example",
        "a..b",
        "[::1]",
        "a_b.example",
    ] {
        assert_eq!(name(unreadable), ClientServerName::Unreadable, "{unreadable:?}");
    }
    let long_label = format!("{}.example", "a".repeat(64));
    assert_eq!(name(&long_label), ClientServerName::Unreadable);
    let long_name = vec!["a".repeat(60); 5].join(".");
    assert_eq!(name(&long_name), ClientServerName::Unreadable);
    let record = build_client_hello_with_sni("api.example.com");
    assert_eq!(
        ClientServerName::of_client_hello(&record[..record.len() - 5]),
        ClientServerName::Unreadable
    );
    assert_eq!(
        ClientServerName::of_client_hello(b"GET / HTTP/1.1\r\n\r\n"),
        ClientServerName::Unreadable
    );
    Ok(())
}

#[test]
fn extracts_sni_from_well_formed_client_hello() -> Result<(), Box<dyn std::error::Error>> {
    let record = build_client_hello_with_sni("api.example.com");
    assert_eq!(
        extract_sni(&record),
        SniExtraction::Found("api.example.com".to_string())
    );
    Ok(())
}

#[test]
fn reports_incomplete_for_partial_record() -> Result<(), Box<dyn std::error::Error>> {
    let record = build_client_hello_with_sni("api.example.com");
    assert_eq!(extract_sni(&record[..record.len() - 5]), SniExtraction::Incomplete);
    assert_eq!(extract_sni(&record[..2]), SniExtraction::Incomplete);
    Ok(())
}

#[test]
fn reports_not_tls_for_plain_http() -> Result<(), Box<dyn std::error::Error>> {
    let buf = b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n";
    assert_eq!(extract_sni(buf), SniExtraction::NotTls);
    Ok(())
}

#[test]
fn reports_not_present_when_no_sni_extension() -> Result<(), Box<dyn std::error::Error>> {
    let mut body = Vec::new();
    body.extend_from_slice(&[0x03, 0x03]);
    body.extend_from_slice(&[0u8; 32]);
    body.push(0);
    body.extend_from_slice(&u16_be(2));
    body.extend_from_slice(&[0x13, 0x01]);
    body.push(1);
    body.push(0);
    body.extend_from_slice(&u16_be(0)); // empty extensions

    let mut handshake = Vec::new();
    handshake.push(HANDSHAKE_TYPE_CLIENT_HELLO);
    let body_len = (body.len() as u32).to_be_bytes();
    handshake.extend_from_slice(&body_len[1..4]);
    handshake.extend_from_slice(&body);

    let mut record = Vec::new();
    record.push(CONTENT_TYPE_HANDSHAKE);
    record.extend_from_slice(&[0x03, 0x01]);
    record.extend_from_slice(&u16_be(handshake.len() as u16));
    record.extend_from_slice(&handshake);

    assert_eq!(extract_sni(&record), SniExtraction::NotPresent);
    Ok(())
}

fn extension(ext_type: u16, data: &[u8]) -> Vec<u8> {
    let mut out = u16_be(ext_type).to_vec();
    out.extend_from_slice(&u16_be(data.len() as u16));
    out.extend_from_slice(data);
    out
}

fn sni_extension(entries: &[(u8, &str)]) -> Vec<u8> {
    let mut list = Vec::new();
    for (name_type, name) in entries {
        list.push(*name_type);
        list.extend_from_slice(&u16_be(name.len() as u16));
        list.extend_from_slice(name.as_bytes());
    }
    let mut data = u16_be(list.len() as u16).to_vec();
    data.extend_from_slice(&list);
    extension(EXTENSION_TYPE_SERVER_NAME, &data)
}

fn client_hello_with_extensions(extensions: &[Vec<u8>]) -> Vec<u8> {
    let extensions = extensions.concat();
    let mut body = vec![0x03, 0x03];
    body.extend_from_slice(&[7u8; 32]);
    body.push(0);
    body.extend_from_slice(&u16_be(2));
    body.extend_from_slice(&[0x13, 0x01]);
    body.extend_from_slice(&[1, 0]);
    body.extend_from_slice(&u16_be(extensions.len() as u16));
    body.extend_from_slice(&extensions);

    let mut handshake = vec![HANDSHAKE_TYPE_CLIENT_HELLO];
    handshake.extend_from_slice(&(body.len() as u32).to_be_bytes()[1..4]);
    handshake.extend_from_slice(&body);

    let mut record = vec![CONTENT_TYPE_HANDSHAKE, 0x03, 0x01];
    record.extend_from_slice(&u16_be(handshake.len() as u16));
    record.extend_from_slice(&handshake);
    record
}

#[test]
fn a_name_is_lower_cased_among_other_extensions() -> Result<(), Box<dyn std::error::Error>> {
    let record = client_hello_with_extensions(&[
        extension(10, &[0x41; 4]),
        sni_extension(&[(0, "API.Example.com")]),
        extension(16, &[0x41; 11]),
    ]);
    assert_eq!(
        extract_sni(&record),
        SniExtraction::Found("api.example.com".to_string())
    );
    Ok(())
}

#[test]
fn other_extensions_alone_carry_no_name() -> Result<(), Box<dyn std::error::Error>> {
    let record = client_hello_with_extensions(&[extension(10, &[0x41; 4])]);
    assert_eq!(extract_sni(&record), SniExtraction::NotPresent);
    Ok(())
}

#[test]
fn a_client_hello_without_a_name_is_absent_and_a_malformed_one_is_unreadable() -> Result<(), Box<dyn std::error::Error>>
{
    let absent = client_hello_with_extensions(&[extension(10, &[0x41; 4])]);
    assert_eq!(ClientServerName::of_client_hello(&absent), ClientServerName::Absent);
    for extensions in [
        vec![sni_extension(&[(0, "a.example"), (0, "b.example")])],
        vec![sni_extension(&[(1, "a.example")])],
        vec![sni_extension(&[(0, "a.example")]), sni_extension(&[(0, "b.example")])],
    ] {
        assert_eq!(
            ClientServerName::of_client_hello(&client_hello_with_extensions(&extensions)),
            ClientServerName::Unreadable
        );
    }
    Ok(())
}

#[test]
fn two_names_a_first_name_of_another_type_or_two_extensions_are_unreadable() -> Result<(), Box<dyn std::error::Error>> {
    for extensions in [
        vec![sni_extension(&[(0, "a.example"), (0, "b.example")])],
        vec![sni_extension(&[(1, "a.example")])],
        vec![sni_extension(&[(0, "a.example")]), sni_extension(&[(0, "b.example")])],
    ] {
        let record = client_hello_with_extensions(&extensions);
        assert_eq!(extract_sni(&record), SniExtraction::Unreadable);
    }
    Ok(())
}

fn cut_at(record: &[u8], keep: usize) -> Vec<u8> {
    let mut cut = record[..RECORD_HEADER_LEN + keep].to_vec();
    cut[3..5].copy_from_slice(&u16_be(keep as u16));
    cut
}

#[test]
fn a_hello_split_across_records_gives_the_name_when_it_falls_before_the_cut() -> Result<(), Box<dyn std::error::Error>>
{
    let early = client_hello_with_extensions(&[sni_extension(&[(0, "a.example")]), extension(21, &[0x41; 400])]);
    let whole = early.len() - RECORD_HEADER_LEN;
    assert_eq!(
        ClientServerName::of_client_hello(&cut_at(&early, whole - 100)),
        ClientServerName::Name("a.example".to_string())
    );
    let late = client_hello_with_extensions(&[extension(21, &[0x41; 400]), sni_extension(&[(0, "a.example")])]);
    let whole = late.len() - RECORD_HEADER_LEN;
    assert_eq!(
        ClientServerName::of_client_hello(&cut_at(&late, whole - 20)),
        ClientServerName::Unreadable
    );
    assert_eq!(
        ClientServerName::of_client_hello(&cut_at(&early, 30)),
        ClientServerName::Unreadable
    );
    Ok(())
}

#[test]
fn a_trailing_dot_and_a_name_outside_ascii_are_unreadable() -> Result<(), Box<dyn std::error::Error>> {
    for host in ["api.example.com.", "b\u{fc}cher.example"] {
        assert_eq!(
            ClientServerName::of_client_hello(&client_hello_with_extensions(&[sni_extension(&[(0, host)])])),
            ClientServerName::Unreadable,
            "{host:?}"
        );
    }
    Ok(())
}

#[test]
fn a_hello_without_any_extensions_carries_no_name() -> Result<(), Box<dyn std::error::Error>> {
    let mut body = vec![0x03, 0x03];
    body.extend_from_slice(&[7u8; 32]);
    body.push(0);
    body.extend_from_slice(&u16_be(2));
    body.extend_from_slice(&[0x13, 0x01]);
    body.extend_from_slice(&[1, 0]);
    let mut handshake = vec![HANDSHAKE_TYPE_CLIENT_HELLO];
    handshake.extend_from_slice(&(body.len() as u32).to_be_bytes()[1..4]);
    handshake.extend_from_slice(&body);
    let mut record = vec![CONTENT_TYPE_HANDSHAKE, 0x03, 0x01];
    record.extend_from_slice(&u16_be(handshake.len() as u16));
    record.extend_from_slice(&handshake);
    assert_eq!(ClientServerName::of_client_hello(&record), ClientServerName::Absent);
    Ok(())
}

#[test]
fn a_hello_from_the_tls_client_of_this_runtime_gives_its_name() -> Result<(), Box<dyn std::error::Error>> {
    crate::tls_ca::install_default_crypto_provider();
    let config = rustls::ClientConfig::builder()
        .with_root_certificates(rustls::RootCertStore::empty())
        .with_no_client_auth();
    let mut connection = rustls::ClientConnection::new(std::sync::Arc::new(config), "Api.Example.com".try_into()?)?;
    let mut first = Vec::new();
    connection.write_tls(&mut first)?;
    let _ = first.flush();
    assert_eq!(
        ClientServerName::of_client_hello(&first),
        ClientServerName::Name("api.example.com".to_string())
    );
    Ok(())
}

#[test]
fn a_name_the_record_cuts_short_is_not_read() -> Result<(), Box<dyn std::error::Error>> {
    let record = client_hello_with_extensions(&[extension(21, &[0x41; 400]), sni_extension(&[(0, "a.example")])]);
    let mut cut = record.clone();
    cut.truncate(record.len() - 20);
    let handshake_len = (cut.len() - RECORD_HEADER_LEN) as u16;
    cut[3..5].copy_from_slice(&u16_be(handshake_len));
    assert_eq!(extract_sni(&cut), SniExtraction::Incomplete);
    Ok(())
}

#[test]
fn a_truncated_record_garbage_or_a_server_hello_carry_no_name() -> Result<(), Box<dyn std::error::Error>> {
    let record = client_hello_with_extensions(&[sni_extension(&[(0, "a.example")])]);
    assert_eq!(extract_sni(&record[..20]), SniExtraction::Incomplete);
    assert_eq!(extract_sni(b"GET / HTTP/1.1\r\n\r\n"), SniExtraction::NotTls);
    let mut server_hello = record;
    server_hello[5] = 2;
    assert_eq!(extract_sni(&server_hello), SniExtraction::NotTls);
    Ok(())
}

fn hello_record(body: &[u8]) -> Vec<u8> {
    let mut handshake = vec![HANDSHAKE_TYPE_CLIENT_HELLO];
    handshake.extend_from_slice(&(body.len() as u32).to_be_bytes()[1..4]);
    handshake.extend_from_slice(body);
    let mut record = vec![CONTENT_TYPE_HANDSHAKE, 0x03, 0x01];
    record.extend_from_slice(&u16_be(handshake.len() as u16));
    record.extend_from_slice(&handshake);
    record
}

fn body_after(tail: &[u8]) -> Vec<u8> {
    let mut body = vec![0x03, 0x03];
    body.extend_from_slice(&[7u8; 32]);
    body.extend_from_slice(tail);
    body
}

#[test]
fn each_missing_field_of_a_hello_is_not_tls() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(extract_sni(&hello_record(&[0x03; 10])), SniExtraction::NotTls);
    assert_eq!(extract_sni(&hello_record(&body_after(&[]))), SniExtraction::NotTls);
    assert_eq!(extract_sni(&hello_record(&body_after(&[5]))), SniExtraction::NotTls);
    assert_eq!(extract_sni(&hello_record(&body_after(&[0]))), SniExtraction::NotTls);
    assert_eq!(
        extract_sni(&hello_record(&body_after(&[0, 0, 10]))),
        SniExtraction::NotTls
    );
    assert_eq!(
        extract_sni(&hello_record(&body_after(&[0, 0, 0]))),
        SniExtraction::NotTls
    );
    assert_eq!(
        extract_sni(&hello_record(&body_after(&[0, 0, 0, 5]))),
        SniExtraction::NotTls
    );
    assert_eq!(
        extract_sni(&hello_record(&body_after(&[0, 0, 0, 1, 0, 0]))),
        SniExtraction::NotTls
    );
    assert_eq!(
        extract_sni(&hello_record(&body_after(&[0, 0, 0, 1, 0, 0, 9, 1]))),
        SniExtraction::NotTls
    );
    Ok(())
}

#[test]
fn a_handshake_shorter_than_its_header_is_incomplete() -> Result<(), Box<dyn std::error::Error>> {
    let record = [CONTENT_TYPE_HANDSHAKE, 0x03, 0x01, 0, 2, HANDSHAKE_TYPE_CLIENT_HELLO, 0];
    assert_eq!(extract_sni(&record), SniExtraction::Incomplete);
    Ok(())
}

#[test]
fn a_server_name_extension_that_cannot_be_read_is_unreadable() -> Result<(), Box<dyn std::error::Error>> {
    let cases = [
        vec![extension(EXTENSION_TYPE_SERVER_NAME, &[0])],
        vec![extension(EXTENSION_TYPE_SERVER_NAME, &[0, 1, 0])],
        vec![extension(EXTENSION_TYPE_SERVER_NAME, &[0, 4, 0, 0, 1, 0xff])],
    ];
    for extensions in cases {
        let record = client_hello_with_extensions(&extensions);
        assert_eq!(extract_sni(&record), SniExtraction::Unreadable);
        assert_eq!(ClientServerName::of_client_hello(&record), ClientServerName::Unreadable);
    }
    Ok(())
}

#[test]
fn an_empty_server_name_list_carries_no_name() -> Result<(), Box<dyn std::error::Error>> {
    let record = client_hello_with_extensions(&[extension(EXTENSION_TYPE_SERVER_NAME, &[0, 0])]);
    assert_eq!(extract_sni(&record), SniExtraction::NotPresent);
    Ok(())
}

#[test]
fn an_extension_longer_than_the_block_that_holds_it_ends_the_scan() -> Result<(), Box<dyn std::error::Error>> {
    let record = client_hello_with_extensions(&[vec![0, 10, 0, 50, 1, 2]]);
    assert_eq!(extract_sni(&record), SniExtraction::NotPresent);
    assert_eq!(ClientServerName::of_client_hello(&record), ClientServerName::Unreadable);
    Ok(())
}

#[test]
fn a_record_cut_inside_the_fixed_fields_gives_no_name() -> Result<(), Box<dyn std::error::Error>> {
    let record = client_hello_with_extensions(&[sni_extension(&[(0, "a.example")])]);
    let fixed = RECORD_HEADER_LEN + HANDSHAKE_HEADER_LEN + 2 + 32;
    assert_eq!(
        ClientServerName::of_client_hello(&cut_at(&record, fixed - RECORD_HEADER_LEN)),
        ClientServerName::Unreadable
    );
    assert_eq!(
        ClientServerName::of_client_hello(&cut_at(&record, fixed - RECORD_HEADER_LEN + 4)),
        ClientServerName::Unreadable
    );
    Ok(())
}
