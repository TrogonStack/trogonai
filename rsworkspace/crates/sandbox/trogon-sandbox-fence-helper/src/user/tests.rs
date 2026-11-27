use super::*;

#[test]
fn password_shape() -> Result<(), Box<dyn std::error::Error>> {
    let p = gen_password()?;
    assert_eq!(p.len(), 32);
    assert!(p.is_ascii());
    // No characters from the excluded set.
    for c in ['"', '\\', '`', ' ', '&', '|', '<', '>', '^'] {
        assert!(!p.contains(c), "password contains '{c}': {p}");
    }
    // At least one from each complexity class (post-seed guarantee).
    assert!(p.bytes().any(|b| b.is_ascii_uppercase()), "{p}");
    assert!(p.bytes().any(|b| b.is_ascii_lowercase()), "{p}");
    assert!(p.bytes().any(|b| b.is_ascii_digit()), "{p}");
    assert!(p.bytes().any(|b| !b.is_ascii_alphanumeric()), "{p}");
    // Two calls differ (overwhelmingly).
    assert_ne!(p, gen_password()?);
    Ok(())
}

#[test]
fn builtin_users_name_resolves() -> Result<(), Box<dyn std::error::Error>> {
    // The exact name is locale-dependent; just check it's
    // non-empty and round-trips back to the well-known SID.
    let name = sid::lookup_account_name(SID_BUILTIN_USERS)?;
    assert!(!name.is_empty());
    let back = sid::lookup_account_sid(&name)?;
    assert_eq!(back, SID_BUILTIN_USERS);
    Ok(())
}

#[test]
fn status_for_absent_user_is_falsey() -> Result<(), Box<dyn std::error::Error>> {
    // The CI smoke installs/uninstalls under the real names, so
    // here we only sanity-check that status() doesn't error and
    // its booleans agree with `exists`.
    let st = status(SANDBOX_USER)?;
    assert_eq!(st.name, SANDBOX_USER);
    if !st.exists {
        assert!(st.sid.is_none());
        assert!(!st.in_builtin_users);
        assert!(!st.in_sandbox_group);
    }
    Ok(())
}
