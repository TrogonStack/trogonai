use super::{JsonDecodeError, JsonDecodeErrorKind};
use crate::python_code_point::PythonCodePoint;

fn located(text: &str, pos: usize) -> (JsonDecodeErrorKind, usize, usize, usize) {
    let text: Vec<PythonCodePoint> = text.chars().map(PythonCodePoint::from).collect();
    let error = JsonDecodeError::new(JsonDecodeErrorKind::ExtraData, &text, pos);
    (error.kind(), error.pos(), error.lineno(), error.colno())
}

#[test]
fn line_and_column_count_from_one_as_json_decode_error_counts_them() {
    assert_eq!(located("", 0), (JsonDecodeErrorKind::ExtraData, 0, 1, 1));
    assert_eq!(located("a\nb", 1), (JsonDecodeErrorKind::ExtraData, 1, 1, 2));
    assert_eq!(located("a\nb", 2), (JsonDecodeErrorKind::ExtraData, 2, 2, 1));
    assert_eq!(
        located("\u{1F600}\n\n\u{e9}x", 4),
        (JsonDecodeErrorKind::ExtraData, 4, 3, 2)
    );
}
