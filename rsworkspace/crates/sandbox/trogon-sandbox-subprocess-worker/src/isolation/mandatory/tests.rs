use super::*;

#[test]
fn parses_a_gitdir_pointer() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(gitdir_pointer(b"gitdir: /a/b\n"), Some("/a/b"));
    assert_eq!(gitdir_pointer(b"gitdir:../x"), Some("../x"));
    Ok(())
}

#[test]
fn refuses_what_is_not_a_pointer() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(gitdir_pointer(b"nonsense"), None);
    assert_eq!(gitdir_pointer(b"gitdir:   \n"), None);
    assert_eq!(gitdir_pointer(&[0xff, 0xfe]), None);
    assert_eq!(gitdir_pointer(b""), None);
    Ok(())
}

#[test]
fn names_match_ignoring_case() -> Result<(), Box<dyn std::error::Error>> {
    assert!(is_dangerous_file(".BashRC"));
    assert!(is_dangerous_directory(".VSCode"));
    assert!(!is_dangerous_file(".bashrc.bak"));
    Ok(())
}

#[test]
fn commondir_is_the_first_line() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(commondir_pointer(b"../..\n"), Some("../.."));
    Ok(())
}
