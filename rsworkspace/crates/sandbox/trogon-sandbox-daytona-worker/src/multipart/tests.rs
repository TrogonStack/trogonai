use super::*;

#[test]
fn the_body_carries_the_path_and_the_content_between_the_boundary() {
    let (boundary, body) = file_part("/tmp/a\"b\\c\nd", b"data");
    let text = String::from_utf8_lossy(&body);
    assert!(text.starts_with(&format!("--{boundary}\r\n")));
    assert!(text.ends_with(&format!("\r\n--{boundary}--\r\n")));
    assert!(text.contains("name=\"files[0].path\"\r\n\r\n/tmp/a\"b\\c\nd\r\n"));
    assert!(text.contains("filename=\"/tmp/a\\\"b\\\\cd\""));
    assert!(text.contains("\r\n\r\ndata\r\n--"));
}
