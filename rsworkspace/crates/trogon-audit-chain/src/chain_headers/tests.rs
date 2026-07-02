use super::*;

#[test]
fn round_trips_valid_headers() {
    let previous_hash = ChainHash::genesis();
    let hash = ChainHash::digest(b"event");

    let mut headers = HeaderMap::new();
    write_chain_headers(&mut headers, &previous_hash, &hash);

    let (read_previous, read_hash) = read_chain_headers(&headers).expect("headers should parse");
    assert_eq!(read_previous, previous_hash);
    assert_eq!(read_hash, hash);
}

#[test]
fn missing_previous_hash_header_is_reported() {
    let mut headers = HeaderMap::new();
    headers.insert(CHAIN_HASH_HEADER, ChainHash::genesis().as_str());

    let err = read_chain_headers(&headers).expect_err("missing header should fail");
    assert_eq!(
        err,
        ChainHeaderError::Missing {
            header_name: CHAIN_PREVIOUS_HASH_HEADER
        }
    );
}

#[test]
fn missing_hash_header_is_reported() {
    let mut headers = HeaderMap::new();
    headers.insert(CHAIN_PREVIOUS_HASH_HEADER, ChainHash::genesis().as_str());

    let err = read_chain_headers(&headers).expect_err("missing header should fail");
    assert_eq!(
        err,
        ChainHeaderError::Missing {
            header_name: CHAIN_HASH_HEADER
        }
    );
}

#[test]
fn forged_header_with_invalid_hash_shape_is_reported() {
    let mut headers = HeaderMap::new();
    headers.insert(CHAIN_PREVIOUS_HASH_HEADER, ChainHash::genesis().as_str());
    headers.insert(CHAIN_HASH_HEADER, "not-a-valid-hash");

    let err = read_chain_headers(&headers).expect_err("invalid hash should fail");
    assert!(matches!(
        err,
        ChainHeaderError::Invalid {
            header_name,
            ..
        } if header_name == CHAIN_HASH_HEADER
    ));
}
