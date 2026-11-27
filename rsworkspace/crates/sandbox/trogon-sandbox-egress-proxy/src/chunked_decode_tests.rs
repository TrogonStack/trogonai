//! Regression tests for inputs the fuzzer found that made the chunked body
//! decoder panic instead of refusing.

use crate::chunked_body::decode_chunked_body;

#[test]
fn chunk_data_without_its_trailing_crlf_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    // The last chunk's data ends the buffer, so the position after the
    // missing CRLF is past the end.
    assert!(decode_chunked_body(b"4\r\nWiki").is_err());
    assert!(decode_chunked_body(b"4\r\nWiki\r").is_err());
    Ok(())
}

#[test]
fn chunk_size_that_overflows_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    assert!(decode_chunked_body(b"ffffffffffffffff\r\nx").is_err());
    Ok(())
}

#[test]
fn well_formed_body_still_decodes() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        decode_chunked_body(b"4\r\nWiki\r\n5;ext=1\r\npedia\r\n0\r\n\r\n")?,
        b"Wikipedia"
    );
    Ok(())
}
