use super::*;

#[test]
fn accepts_only_one_through_eight_kibibytes() {
    assert_eq!(RenderedUtf8Bytes::new(0), Err(PeerValueError::InvalidRenderedSize));
    assert_eq!(RenderedUtf8Bytes::new(1).map(RenderedUtf8Bytes::get), Ok(1));
    assert_eq!(RenderedUtf8Bytes::new(8192).map(RenderedUtf8Bytes::get), Ok(8192));
    assert_eq!(RenderedUtf8Bytes::new(8193), Err(PeerValueError::InvalidRenderedSize));
}
