use super::*;

#[test]
fn each_encoding_names_its_media_type() {
    assert_eq!(PayloadEncoding::Json.content_type(), "application/json");
    assert_eq!(
        PayloadEncoding::Form.content_type(),
        "application/x-www-form-urlencoded"
    );
}
