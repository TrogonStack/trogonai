use super::*;

#[test]
fn psid_string_round_trip() -> Result<(), Box<dyn std::error::Error>> {
    // Well-known Everyone SID.
    let p = LocalPsid::from_string("S-1-1-0")?;
    let s = psid_to_string(p.as_psid())?;
    assert_eq!(s, "S-1-1-0");
    Ok(())
}

#[test]
fn lookup_builtin_users() -> Result<(), Box<dyn std::error::Error>> {
    // BUILTIN\Users exists on every Windows install.
    let s = lookup_account_sid("BUILTIN\\Users")?;
    assert_eq!(s, "S-1-5-32-545");
    Ok(())
}

#[test]
fn lookup_missing_account_errors() {
    let r = lookup_account_sid("no-such-group-fence-helper-test");
    assert!(r.is_err());
}

#[test]
fn current_user_sid_is_nonempty() -> Result<(), Box<dyn std::error::Error>> {
    let s = current_user_sid()?;
    assert!(s.starts_with("S-1-"), "got {s}");
    Ok(())
}
