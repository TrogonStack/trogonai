pub(crate) fn is_token(text: &str) -> bool {
    !text.is_empty()
        && text.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                        | b'-'
                )
        })
}

pub(crate) fn is_header_value(text: &str) -> bool {
    text.chars().all(|c| {
        let code = u32::from(c);
        !((code < 0x20 && code != 0x09) || code == 0x7f || code > 0xff)
    })
}

/// A header name folded for comparison: lower case, with `_` and `.` read as
/// `-`, because upstreams differ in which separators they treat as equal.
pub fn fold_header_name(name: &str) -> String {
    name.to_ascii_lowercase().replace(['_', '.'], "-")
}
