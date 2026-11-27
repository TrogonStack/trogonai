use super::*;
use trogon_sandbox_contract::StrippedHeader;

fn strip(names: &[&str]) -> Result<ResponseHeaderStrip, Box<dyn std::error::Error>> {
    Ok(ResponseHeaderStrip::new(
        names
            .iter()
            .map(|n| StrippedHeader::new(*n))
            .collect::<Result<Vec<_>, _>>()?,
    ))
}

#[test]
fn an_empty_strip_leaves_the_head_untouched() -> Result<(), Box<dyn std::error::Error>> {
    let head = "HTTP/1.1 200 OK\r\nSet-Cookie: a=1\r\n\r\n";
    assert_eq!(strip_response_headers(head, &strip(&[])?), head);
    Ok(())
}

#[test]
fn every_occurrence_of_a_stripped_header_is_removed_in_any_spelling() -> Result<(), Box<dyn std::error::Error>> {
    let head = "HTTP/1.1 200 OK\r\nSet-Cookie: a=1\r\nX-Kept: yes\r\nset_cookie: b=2\r\nContent-Length: 0\r\n\r\n";
    assert_eq!(
        strip_response_headers(head, &strip(&["set-cookie"])?),
        "HTTP/1.1 200 OK\r\nX-Kept: yes\r\nContent-Length: 0\r\n\r\n"
    );
    Ok(())
}

#[test]
fn folded_continuation_lines_go_with_their_header() -> Result<(), Box<dyn std::error::Error>> {
    let head = "HTTP/1.1 200 OK\r\nSet-Cookie: a=1;\r\n  Path=/\r\nX-Kept: yes\r\n  more\r\n\r\n";
    assert_eq!(
        strip_response_headers(head, &strip(&["set-cookie"])?),
        "HTTP/1.1 200 OK\r\nX-Kept: yes\r\n  more\r\n\r\n"
    );
    Ok(())
}

#[test]
fn a_header_name_is_matched_whole_not_as_a_prefix() -> Result<(), Box<dyn std::error::Error>> {
    let head = "HTTP/1.1 200 OK\r\nSet-Cookie2: a\r\nX-Set-Cookie: b\r\n\r\n";
    assert_eq!(strip_response_headers(head, &strip(&["set-cookie"])?), head);
    Ok(())
}
