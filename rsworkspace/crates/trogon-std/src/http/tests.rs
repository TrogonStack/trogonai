use super::*;

#[test]
fn new_converts_correctly() {
    let size = HttpBodySizeMax::new(ByteSize::mib(25)).unwrap();
    assert_eq!(size.as_usize(), 25 * 1024 * 1024);
}

#[test]
fn new_one_mib() {
    let size = HttpBodySizeMax::new(ByteSize::mib(1)).unwrap();
    assert_eq!(size.as_usize(), 1024 * 1024);
}

#[test]
fn new_zero_returns_none() {
    assert!(HttpBodySizeMax::new(ByteSize::b(0)).is_none());
}
