use super::*;

#[test]
fn a_well_formed_head_is_accepted_in_some_mode() -> Result<(), Box<dyn std::error::Error>> {
    assert!(proxied_head(
        b"GET http://example.test/ HTTP/1.1\r\nHost: example.test\r\n\r\n"
    ));
    Ok(())
}

#[test]
fn garbage_is_refused_without_a_panic() -> Result<(), Box<dyn std::error::Error>> {
    assert!(!proxied_head(b"\x00\xff not a request"));
    Ok(())
}

#[test]
fn a_chunked_body_decodes() -> Result<(), Box<dyn std::error::Error>> {
    assert!(chunked_body(b"3\r\nabc\r\n0\r\n\r\n"));
    Ok(())
}

#[test]
fn a_broken_chunked_body_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    assert!(!chunked_body(b"zz\r\nabc"));
    assert!(!chunked_body(b""));
    Ok(())
}
