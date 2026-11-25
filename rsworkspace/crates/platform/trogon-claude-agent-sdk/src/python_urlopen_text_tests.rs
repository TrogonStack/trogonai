use super::*;
use crate::constants::READ_SIZE_OVERFLOW;

type Base64Case = (&'static [u8], Result<&'static [u8], &'static str>);

#[test]
fn parse_int_matches_python_int_of_a_str() {
    let cases: [(&str, Option<&str>); 12] = [
        (" 12 ", Some("12")),
        ("-0", Some("0")),
        ("-007", Some("-7")),
        ("+1_000", Some("1000")),
        ("1__0", None),
        ("_1", None),
        ("1_", None),
        ("", None),
        ("\u{663}", Some("3")),
        ("\u{2003}7\u{2003}", Some("7")),
        ("0x10", None),
        ("1\u{e9}", None),
    ];
    for (text, expected) in cases {
        assert_eq!(
            parse_int(text).map(|value| value.decimal()).as_deref(),
            expected,
            "{text:?}"
        );
    }
    assert_eq!(parse_int(&"1".repeat(PYTHON_INT_MAX_STR_DIGITS + 1)), None);
    assert_eq!(
        parse_int(&"1".repeat(PYTHON_INT_MAX_STR_DIGITS)).map(|value| value.decimal().len()),
        Some(PYTHON_INT_MAX_STR_DIGITS)
    );
    assert_eq!(Some(PythonInt::zero()), parse_int("-0"));
}

#[test]
fn python_int_fits_a_c_ssize_t_or_not() {
    let cases: [(&str, Option<i64>); 6] = [
        ("9223372036854775807", Some(i64::MAX)),
        ("9223372036854775808", None),
        ("-9223372036854775808", Some(i64::MIN)),
        ("-9223372036854775809", None),
        ("123456789012345678901234567890", None),
        ("-5", Some(-5)),
    ];
    for (text, expected) in cases {
        assert_eq!(parse_int(text).and_then(|value| value.to_i64()), expected, "{text:?}");
    }
}

#[test]
fn parse_hex_bytes_matches_python_int_of_bytes_in_base_16() {
    let cases: [(&[u8], Option<ReadSize>); 15] = [
        (b"1a", Some(ReadSize::Bytes(26))),
        (b" 0x_1F \r\n", Some(ReadSize::Bytes(31))),
        (b"0X1f", Some(ReadSize::Bytes(31))),
        (b"+f", Some(ReadSize::Bytes(15))),
        (b"-1", Some(ReadSize::Bytes(-1))),
        (b"1_0", Some(ReadSize::Bytes(16))),
        (b"1;ext", None),
        (b"\x00", None),
        (b"", None),
        (b"0x", None),
        (b"7fffffffffffffff", Some(ReadSize::Bytes(i64::MAX))),
        (b"-8000000000000000", Some(ReadSize::Bytes(i64::MIN))),
        (b"8000000000000000", Some(ReadSize::Above)),
        (b"-8000000000000001", Some(ReadSize::Below)),
        (b"-0", Some(ReadSize::Bytes(0))),
    ];
    for (line, expected) in cases {
        assert_eq!(parse_hex_bytes(line), expected, "{line:?}");
    }
    assert_eq!(parse_hex_bytes(&b"f".repeat(64)), Some(ReadSize::Above));
    assert_eq!(
        parse_hex_bytes(&[b"-".as_slice(), &b"f".repeat(64)].concat()),
        Some(ReadSize::Below)
    );
    assert_eq!(READ_SIZE_OVERFLOW, "cannot fit 'int' into an index-sized integer");
}

#[test]
fn a2b_base64_matches_binascii_without_strict_mode() {
    let cases: [Base64Case; 9] = [
        (b"aGVsbG8=", Ok(b"hello")),
        (b"aGVsbG8", Err("Incorrect padding")),
        (
            b"a",
            Err("Invalid base64-encoded string: number of data characters (1) cannot be 1 more than a multiple of 4"),
        ),
        (
            b"aGVsb",
            Err("Invalid base64-encoded string: number of data characters (5) cannot be 1 more than a multiple of 4"),
        ),
        (b"aGk=garbage", Ok(b"hi")),
        (b"=aGk=", Ok(b"hi")),
        (b"a=Gk=", Ok(b"hi")),
        (b"aG\nk*=", Ok(b"hi")),
        (b"+/9=", Ok(b"\xfb\xff")),
    ];
    for (data, expected) in cases {
        let decoded = a2b_base64(data);
        assert_eq!(decoded.as_deref().map_err(String::as_str), expected, "{data:?}");
    }
    assert_eq!(a2b_base64(b"").as_deref(), Ok(&b""[..]));
}

#[test]
fn unquote_to_bytes_matches_python() {
    let cases: [(&str, &[u8]); 7] = [
        ("a%20b", b"a b"),
        ("%zz", b"%zz"),
        ("%", b"%"),
        ("%4", b"%4"),
        ("\u{e9}%C3%A9", b"\xc3\xa9\xc3\xa9"),
        ("", b""),
        ("%41%4", b"A%4"),
    ];
    for (text, expected) in cases {
        assert_eq!(unquote_to_bytes(text), expected, "{text:?}");
    }
}

#[test]
fn bytes_repr_matches_python() {
    assert_eq!(bytes_repr(b"a'b"), "b\"a'b\"");
    assert_eq!(bytes_repr(b"a'\"b"), "b'a\\'\"b'");
    assert_eq!(bytes_repr(b"\\\t\n\r\x00\x7f\xff"), "b'\\\\\\t\\n\\r\\x00\\x7f\\xff'");
}

#[test]
fn encode_matches_the_ascii_and_latin_1_codecs() {
    assert_eq!(encode("abc", "ascii", 128).as_deref(), Ok(&b"abc"[..]));
    assert_eq!(encode("\u{e9}", "latin-1", 256).as_deref(), Ok(&b"\xe9"[..]));
    let cases: [(&str, &str, u32, &str); 4] = [
        (
            "a\u{e9}",
            "ascii",
            128,
            "'ascii' codec can't encode character '\\xe9' in position 1: ordinal not in range(128)",
        ),
        (
            "a\u{e9}\u{e9}b",
            "ascii",
            128,
            "'ascii' codec can't encode characters in position 1-2: ordinal not in range(128)",
        ),
        (
            "\u{20ac}",
            "latin-1",
            256,
            "'latin-1' codec can't encode character '\\u20ac' in position 0: ordinal not in range(256)",
        ),
        (
            "\u{1f600}",
            "latin-1",
            256,
            "'latin-1' codec can't encode character '\\U0001f600' in position 0: ordinal not in range(256)",
        ),
    ];
    for (text, codec, limit, expected) in cases {
        assert_eq!(encode(text, codec, limit), Err(expected.to_owned()), "{text:?}");
    }
}

#[test]
fn latin1_maps_each_byte_to_its_code_point() {
    assert_eq!(latin1(b"a\xe9\xff"), "a\u{e9}\u{ff}");
}

#[test]
fn os_error_class_names_the_python_subclass() {
    let cases = [
        (io::ErrorKind::ConnectionReset, "ConnectionResetError"),
        (io::ErrorKind::ConnectionAborted, "ConnectionAbortedError"),
        (io::ErrorKind::ConnectionRefused, "ConnectionRefusedError"),
        (io::ErrorKind::BrokenPipe, "BrokenPipeError"),
        (io::ErrorKind::TimedOut, "TimeoutError"),
        (io::ErrorKind::WouldBlock, "BlockingIOError"),
        (io::ErrorKind::Interrupted, "InterruptedError"),
        (io::ErrorKind::NotFound, "FileNotFoundError"),
        (io::ErrorKind::PermissionDenied, "PermissionError"),
        (io::ErrorKind::AlreadyExists, "FileExistsError"),
        (io::ErrorKind::IsADirectory, "IsADirectoryError"),
        (io::ErrorKind::NotADirectory, "NotADirectoryError"),
        (io::ErrorKind::InvalidData, "OSError"),
    ];
    for (kind, expected) in cases {
        assert_eq!(os_error_class(&io::Error::from(kind)), expected, "{kind:?}");
    }
}

#[cfg(unix)]
#[test]
fn os_error_text_matches_python_str_of_an_os_error() {
    let missing = io::Error::from_raw_os_error(2);
    assert_eq!(strerror(2), "No such file or directory");
    assert_eq!(os_error_text(&missing, None), "[Errno 2] No such file or directory");
    assert_eq!(
        os_error_text(&missing, Some("/x")),
        "[Errno 2] No such file or directory: '/x'"
    );
}

#[test]
fn os_error_text_without_an_errno_is_the_error_text() {
    assert_eq!(os_error_text(&io::Error::other("boom"), None), "boom");
}
