use super::*;

#[test]
fn from_secs_valid() {
    let d = NonZeroDuration::from_secs(10).unwrap();
    assert_eq!(Duration::from(d), Duration::from_secs(10));
}

#[test]
fn from_secs_zero_rejected() {
    assert!(matches!(NonZeroDuration::from_secs(0), Err(ZeroDurationError)));
}

#[test]
fn from_millis_valid() {
    let d = NonZeroDuration::from_millis(500).unwrap();
    assert_eq!(Duration::from(d), Duration::from_millis(500));
}

#[test]
fn from_millis_zero_rejected() {
    assert!(matches!(NonZeroDuration::from_millis(0), Err(ZeroDurationError)));
}

#[test]
fn error_display() {
    assert_eq!(ZeroDurationError.to_string(), "duration must not be zero");
}

#[test]
fn copy_semantics() {
    let a = NonZeroDuration::from_secs(5).unwrap();
    let b = a;
    assert_eq!(a, b);
}

#[test]
fn ordering() {
    let short = NonZeroDuration::from_secs(1).unwrap();
    let long = NonZeroDuration::from_secs(10).unwrap();
    assert!(short < long);
}
