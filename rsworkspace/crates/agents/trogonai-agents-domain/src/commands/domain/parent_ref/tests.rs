use super::*;

#[test]
fn accepts_nonblank_trimmed_parent_ref() {
    assert_eq!(ParentRef::parse("org/root").unwrap().as_str(), "org/root");
}

#[test]
fn rejects_blank_or_untrimmed_parent_ref() {
    assert!(ParentRef::parse("").is_err());
    assert!(ParentRef::parse(" org/root").is_err());
}

#[test]
fn supports_standard_string_conversions_and_display() {
    let value = ParentRef::parse("org/root").unwrap();

    assert_eq!(value.as_ref(), "org/root");
    assert_eq!("org/root".parse::<ParentRef>().unwrap().as_str(), "org/root");
    assert_eq!(value.to_string(), "org/root");
}
