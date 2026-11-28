use super::*;
use crate::wfp::DEFAULT_SUBLAYER_GUID;

const A: &str = "2c5d0ad6-5f3b-4d4e-9b8f-1a3e7c9d0b21";
const B: &str = "7d3c1a52-6e0b-4f8a-b1d4-2a9c5e7f3b60";

fn modes(v: &[&str]) -> Vec<(String, String)> {
    parse_loopback_modes(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>())
}

#[test]
fn sublayer_key_is_the_lowercase_guid() {
    assert_eq!(sublayer_key(&DEFAULT_SUBLAYER_GUID), A);
}

#[test]
fn one_sublayer_changes_only_its_own_line() {
    let both = [format!("{A}=closed"), format!("{B}=open")];
    let got = loopback_modes_with(modes(&[&both[0], &both[1]]), A, Some(MODE_OPEN));
    assert_eq!(got, [format!("{B}=open"), format!("{A}=open")]);
    // An uninstall removes the line and leaves the other.
    let got = loopback_modes_with(modes(&[&both[0], &both[1]]), A, None);
    assert_eq!(got, [format!("{B}=open")]);
    // A key is matched whatever its case; a line without `=` is dropped.
    let upper = format!("{}=open", A.to_uppercase());
    assert_eq!(modes(&[&upper, "junk"]), modes(&[&format!("{A}=open")]));
}

#[test]
fn loopback_is_open_only_when_every_recorded_set_is() -> Result<(), Box<dyn std::error::Error>> {
    // No record: nothing installed, or a set from before the record.
    assert!(!effective_loopback(&[])?);
    assert!(effective_loopback(&modes(&[&format!("{A}=open")]))?);
    assert!(!effective_loopback(&modes(&[&format!("{A}=closed")]))?);
    // One closed set's BLOCK applies to every sandboxed connect.
    let mixed = modes(&[&format!("{A}=open"), &format!("{B}=closed")]);
    assert!(!effective_loopback(&mixed)?);
    Ok(())
}

#[test]
fn a_mode_that_is_changing_or_unknown_is_an_error_not_closed() {
    for word in [MODE_CHANGING, "1", ""] {
        let m = modes(&[&format!("{A}=open"), &format!("{B}={word}")]);
        assert!(effective_loopback(&m).is_err(), "{word:?}");
    }
}

#[test]
fn only_this_builds_marker_version_passes() -> Result<(), Box<dyn std::error::Error>> {
    assert!(check_marker(SETUP_VERSION).is_ok());
    // Only a newer install is refused; an older one is upgraded.
    assert!(newer_setup_refusal(SETUP_VERSION).is_none());
    assert!(newer_setup_refusal(SETUP_VERSION - 1).is_none());
    let why = newer_setup_refusal(SETUP_VERSION + 1).ok_or("missing value")?;
    assert!(why.contains("newer than this build's"), "{why}");
    for other in [0, SETUP_VERSION - 1, SETUP_VERSION + 1, 99] {
        let e = check_marker(other).err().ok_or("expected an error")?.to_string();
        assert!(e.contains("marker version mismatch"), "{e}");
        assert!(e.contains("re-run `fence-helper install`"), "{e}");
    }
    Ok(())
}

#[test]
fn an_unreadable_ambient_record_is_an_error_not_an_absent_target() {
    let read = Ok(vec![r"C:\ProgramData".to_owned()]);
    assert!(matches!(ambient_deny_recorded_in(read, r"C:\ProgramData"), Ok(true)));
    let read = Ok(vec![r"C:\ProgramData".to_owned()]);
    assert!(matches!(ambient_deny_recorded_in(read, r"C:\Other"), Ok(false)));
    let unreadable = Err(crate::error::FenceHelperError::msg("registry unreadable"));
    assert!(ambient_deny_recorded_in(unreadable, r"C:\ProgramData").is_err());
}
