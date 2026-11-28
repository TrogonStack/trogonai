//! Lowercase hexadecimal, the one spelling digests and opaque ids are written in.
//!
//! Reading is laxer than writing: [`decode`] takes either case, because some of
//! the text it reads was spelled by other programs. A caller whose format is
//! canonical lowercase checks that itself before decoding.

/// Writes each byte as two lowercase hexadecimal digits.
#[must_use]
pub fn encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(char::from(DIGITS[usize::from(byte >> 4)]));
        text.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    text
}

/// Reads the bytes back from two hexadecimal digits each, in either case.
///
/// `None` for an odd number of digits or for anything that is not a digit: a
/// sign, whitespace and a `0x` prefix are all refused rather than skipped.
#[must_use]
pub fn decode(text: &str) -> Option<Vec<u8>> {
    let (pairs, rest) = text.as_bytes().as_chunks::<2>();
    if !rest.is_empty() {
        return None;
    }
    pairs.iter().map(decode_pair).collect()
}

/// Reads exactly `N` bytes back, under the same rules as [`decode`].
///
/// `None` as well when the text is not exactly `2 * N` digits long.
#[must_use]
pub fn decode_array<const N: usize>(text: &str) -> Option<[u8; N]> {
    let (pairs, rest) = text.as_bytes().as_chunks::<2>();
    if pairs.len() != N || !rest.is_empty() {
        return None;
    }
    let mut bytes = [0_u8; N];
    for (byte, pair) in bytes.iter_mut().zip(pairs) {
        *byte = decode_pair(pair)?;
    }
    Some(bytes)
}

fn decode_pair(pair: &[u8; 2]) -> Option<u8> {
    Some((digit(pair[0])? << 4) | digit(pair[1])?)
}

fn digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{decode, decode_array, encode};

    #[test]
    fn every_byte_becomes_two_lowercase_digits() {
        assert_eq!(encode(&[]), "");
        assert_eq!(encode(&[0x00, 0x0f, 0xa5, 0xff]), "000fa5ff");
    }

    #[test]
    fn decoding_reads_back_what_encoding_wrote() {
        let bytes: Vec<u8> = (0..=u8::MAX).collect();
        assert_eq!(decode(&encode(&bytes)), Some(bytes));
        assert_eq!(decode(""), Some(Vec::new()));
    }

    #[test]
    fn decoding_accepts_either_case() {
        assert_eq!(decode("000fA5ff"), Some(vec![0x00, 0x0f, 0xa5, 0xff]));
        assert_eq!(decode("ABCDEF"), decode("abcdef"));
    }

    #[test]
    fn an_odd_number_of_digits_is_refused() {
        assert_eq!(decode("a"), None);
        assert_eq!(decode("abc"), None);
    }

    #[test]
    fn anything_that_is_not_a_digit_is_refused() {
        for text in ["0g", "+f", "-1", " 1", "0x", "\u{e9}", "g0"] {
            assert_eq!(decode(text), None, "{text:?}");
        }
    }

    #[test]
    fn an_array_is_read_only_from_exactly_its_length() {
        assert_eq!(decode_array::<2>("0fA5"), Some([0x0f, 0xa5]));
        assert_eq!(decode_array::<2>("0f"), None);
        assert_eq!(decode_array::<2>("0fa5ff"), None);
        assert_eq!(decode_array::<2>("0fa"), None);
        assert_eq!(decode_array::<2>("0fa5f"), None);
        assert_eq!(decode_array::<2>("0fzz"), None);
        assert_eq!(decode_array::<0>(""), Some([]));
    }
}
