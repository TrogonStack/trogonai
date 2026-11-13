use super::*;

#[test]
fn refuses_an_empty_name() {
    assert_eq!(OpenBaoNameError::check(""), Err(OpenBaoNameError::Empty));
}

#[test]
fn refuses_a_path_separator() {
    assert_eq!(
        OpenBaoNameError::check("release/../sys"),
        Err(OpenBaoNameError::InvalidCharacter('/'))
    );
}

#[test]
fn refuses_dot_segments() {
    assert_eq!(OpenBaoNameError::check("."), Err(OpenBaoNameError::DotSegment));
    assert_eq!(OpenBaoNameError::check(".."), Err(OpenBaoNameError::DotSegment));
}

#[test]
fn admits_the_name_alphabet() {
    assert_eq!(OpenBaoNameError::check("secrets-service_release.read1"), Ok(()));
}
