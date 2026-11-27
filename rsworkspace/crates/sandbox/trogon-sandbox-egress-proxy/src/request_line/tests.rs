use super::*;

fn read(line: &str) -> Result<RequestLine<'_>, HeadParseError> {
    RequestLine::read(line, LineEnd::Crlf)
}

#[test]
fn the_method_is_matched_the_way_the_parser_walks_its_table() -> Result<(), Box<dyn std::error::Error>> {
    for (line, error) in [
        ("GE / HTTP/1.1", HeadParseError::InvalidMethod),
        ("GET_X / HTTP/1.1", HeadParseError::InvalidMethod),
        ("get / HTTP/1.1", HeadParseError::InvalidMethod),
        ("TRACK / HTTP/1.1", HeadParseError::InvalidMethod),
        ("FOO / HTTP/1.1", HeadParseError::InvalidMethod),
        ("PR / HTTP/1.1", HeadParseError::InvalidMethod),
        ("PRIX / HTTP/1.1", HeadParseError::ExpectedSpaceAfterMethod),
        ("GETX / HTTP/1.1", HeadParseError::ExpectedSpaceAfterMethod),
        ("GET", HeadParseError::ExpectedSpaceAfterMethod),
        ("GET\t/ HTTP/1.1", HeadParseError::ExpectedSpaceAfterMethod),
        ("GET  ", HeadParseError::InvalidUrl),
        ("GET /\rHost: a", HeadParseError::InvalidUrl),
    ] {
        assert_eq!(read(line), Err(error), "{line}");
    }
    Ok(())
}

#[test]
fn each_protocol_admits_its_own_methods() -> Result<(), Box<dyn std::error::Error>> {
    for (line, error) in [
        ("DESCRIBE / HTTP/1.1", HeadParseError::InvalidMethodForHttp),
        ("HEAD / RTSP/1.0", HeadParseError::InvalidMethodForRtsp),
        ("SOURCE / RTSP/1.0", HeadParseError::InvalidMethodForRtsp),
        ("GET / ICE/1.0", HeadParseError::ExpectedSourceForIce),
        ("DESCRIBE / HTTP/9.9", HeadParseError::InvalidMethodForHttp),
        ("PRI / RTSP/1.0", HeadParseError::InvalidMethodForRtsp),
        ("PRI / ICE/1.0", HeadParseError::ExpectedSourceForIce),
        ("PRI / HTTP/3.0", HeadParseError::InvalidVersion),
        ("PRI * HTTP/2.0 x", HeadParseError::ExpectedPreface),
    ] {
        assert_eq!(read(line), Err(error), "{line}");
    }
    for line in [
        "DESCRIBE / RTSP/1.0",
        "GET_PARAMETER / RTSP/1.0",
        "POST / RTSP/1.0",
        "SOURCE / ICE/1.0",
        "SOURCE / HTTP/1.1",
    ] {
        assert!(read(line).is_ok(), "{line}");
    }
    Ok(())
}

#[test]
fn the_version_is_read_digit_by_digit_then_checked() -> Result<(), Box<dyn std::error::Error>> {
    for (line, error) in [
        ("GET / ", HeadParseError::ExpectedProtocol),
        ("GET / http/1.1", HeadParseError::ExpectedProtocol),
        ("GET / HTTP/ ", HeadParseError::InvalidMajorVersion),
        ("GET / HTTP/x.1", HeadParseError::InvalidMajorVersion),
        ("GET / HTTP/1", HeadParseError::ExpectedDot),
        ("GET / HTTP/1.", HeadParseError::InvalidMinorVersion),
        ("GET / HTTP/1.x", HeadParseError::InvalidMinorVersion),
        ("GET / HTTP/1.2 ", HeadParseError::InvalidVersion),
        ("GET / HTTP/3.0", HeadParseError::InvalidVersion),
        ("GET / RTSP/1.2", HeadParseError::InvalidVersion),
        ("GET / HTTP/1.12", HeadParseError::ExpectedCrlf),
        ("GET / HTTP/1.1 x", HeadParseError::ExpectedCrlf),
        ("GET / HTTP/2.0 ", HeadParseError::ExpectedCrlf),
    ] {
        assert_eq!(read(line), Err(error), "{line}");
    }
    Ok(())
}

#[test]
fn only_x_1_is_answered_as_http_1_1() -> Result<(), Box<dyn std::error::Error>> {
    for (line, version) in [
        ("GET / HTTP/1.1", RequestVersion::Http11),
        ("GET / RTSP/1.1", RequestVersion::Http11),
        ("GET / HTTP/1.0", RequestVersion::Http10),
        ("GET / HTTP/2.0", RequestVersion::Http10),
        ("GET / HTTP/0.9", RequestVersion::Http10),
        ("GET /", RequestVersion::Http10),
        ("DESCRIBE /", RequestVersion::Http10),
    ] {
        assert_eq!(read(line).map(|line| line.version), Ok(version), "{line}");
    }
    Ok(())
}

#[test]
fn spaces_around_the_target_are_skipped() -> Result<(), Box<dyn std::error::Error>> {
    let line = read("GET   /a  HTTP/1.1")?;
    assert_eq!((line.method, line.target), ("GET", "/a"));
    Ok(())
}

#[test]
fn a_bare_line_feed_ends_only_a_line_without_a_protocol() -> Result<(), Box<dyn std::error::Error>> {
    assert!(RequestLine::read("GET /", LineEnd::BareLf).is_ok());
    assert_eq!(
        RequestLine::read("GET / HTTP/1.1", LineEnd::BareLf),
        Err(HeadParseError::ExpectedCrlf)
    );
    Ok(())
}

#[test]
fn a_pri_line_naming_a_protocol_is_followed_by_the_http_2_preface() -> Result<(), Box<dyn std::error::Error>> {
    for (bytes, preface) in [
        (
            &b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n"[..],
            Some(Preface::Refused(HeadParseError::PausedUpgrade)),
        ),
        (
            b"PRI / HTTP/1.1\r\n\r\nSM\r\n\r\nrest",
            Some(Preface::Refused(HeadParseError::PausedUpgrade)),
        ),
        (b"PRI * HTTP/2.0\r\n\r\nS", Some(Preface::Partial)),
        (b"PRI * HTTP/2.0\r\n", Some(Preface::Partial)),
        (
            b"PRI / HTTP/1.1\r\nHost: a\r\n\r\n",
            Some(Preface::Refused(HeadParseError::ExpectedPreface)),
        ),
        (
            b"PRI * HTTP/2.0\n\r\nSM\r\n\r\n",
            Some(Preface::Refused(HeadParseError::ExpectedPreface)),
        ),
        (b"PRI * HTTP/2.0", None),
        (b"PRI /\r\n\r\n", None),
        (b"GET / HTTP/1.1\r\n\r\n", None),
    ] {
        assert_eq!(Preface::of(bytes), preface, "{bytes:?}");
    }
    Ok(())
}

#[test]
fn a_pri_line_without_a_protocol_is_a_simple_request() -> Result<(), Box<dyn std::error::Error>> {
    let line = read("PRI /")?;
    assert_eq!((line.method, line.form), (PRI, LineForm::Simple));
    Ok(())
}
