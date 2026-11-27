use super::*;
use windows::Win32::Foundation::CloseHandle;

#[test]
fn restricted_token_builds() -> Result<(), Box<dyn std::error::Error>> {
    let base = open_self_token()?;
    let r = make_sandbox_token(base, false, &[]);
    // Capability SIDs are refused without the flag; other SIDs with it.
    let cap = crate::persist::new_capability_sid()?;
    assert!(make_sandbox_token(base, false, std::slice::from_ref(&cap)).is_err());
    assert!(make_sandbox_token(base, true, &[SID_BUILTIN_ADMINS.to_owned()]).is_err());
    unsafe {
        let _ = CloseHandle(base);
    }
    let tok = r?;
    let shape = token_restriction(tok)?;
    assert!(!shape.write_restricted);
    assert_eq!(shape.restricted_sids, Vec::<String>::new());
    let prim = to_primary(tok)?;
    unsafe {
        let _ = CloseHandle(tok);
        let _ = CloseHandle(prim);
    }
    Ok(())
}

#[test]
fn token_group_sids_reads_a_buffer_shorter_than_the_struct() -> Result<(), Box<dyn std::error::Error>> {
    // What the kernel returns for no groups can be the count alone.
    assert!(token_group_sids(&0u32.to_ne_bytes())?.is_empty());
    assert!(token_group_sids(&[0u8; 2]).is_err());
    // A count the buffer does not hold is refused, not read.
    let mut one = 1u32.to_ne_bytes().to_vec();
    assert!(token_group_sids(&one).is_err());
    one.resize(
        std::mem::offset_of!(TOKEN_GROUPS, Groups) + size_of::<SID_AND_ATTRIBUTES>(),
        0,
    );
    assert_eq!(token_group_sids(&one)?.len(), 1);
    Ok(())
}

/// The user SID must stay in the list: the WFP egress BLOCK matches through it.
#[test]
fn write_restricted_shape_is_pinned() {
    assert_eq!(restricted_token_flags(false), LUA_TOKEN);
    assert_eq!(restricted_token_flags(false).0, 0x4);
    assert_eq!(restricted_token_flags(true).0, 0x4 | 0x8);
    assert_eq!(WRITE_RESTRICTED.0, 0x8);
    assert_eq!(
        restricting_sid_list("S-1-5-21-1-2-3-1005", &[]),
        ["S-1-5-21-1-2-3-1005", "S-1-1-0", "S-1-5-12"]
    );
    let (a, b) = (
        "S-1-0-1953719907-1-2-3-4".to_owned(),
        "S-1-0-1953719907-5-6-7-8".to_owned(),
    );
    assert_eq!(
        restricting_sid_list("S-1-5-21-1-2-3-1005", &[b.clone(), a.clone(), b.clone()]),
        [b.as_str(), a.as_str(), "S-1-5-21-1-2-3-1005", "S-1-1-0", "S-1-5-12"]
    );
}

/// The kernel reports the requested list and flag, also after `to_primary`.
#[test]
fn write_restricted_token_reports_exact_list() -> Result<(), Box<dyn std::error::Error>> {
    let caps = [
        crate::persist::new_capability_sid()?,
        crate::persist::new_capability_sid()?,
    ];
    for caps in [&caps[..0], &caps[..]] {
        let base = open_self_token()?;
        let user = token_user_sid(base)?;
        let r = make_sandbox_token(base, true, caps);
        unsafe {
            let _ = CloseHandle(base);
        }
        let tok = r?;
        let prim = to_primary(tok)?;
        for h in [tok, prim] {
            let shape = token_restriction(h)?;
            assert!(shape.write_restricted, "TOKEN_WRITE_RESTRICTED not set");
            assert_eq!(shape.restricted_sids, restricting_sid_list(&user, caps));
        }
        unsafe {
            let _ = CloseHandle(tok);
            let _ = CloseHandle(prim);
        }
    }
    Ok(())
}

#[test]
fn privileges_except_keeps_change_notify() -> Result<(), Box<dyn std::error::Error>> {
    let base = open_self_token()?;
    let to_delete = privileges_except(base, &["SeChangeNotifyPrivilege"])?;
    // Resolve SeChangeNotifyPrivilege's LUID and assert it's NOT
    // in the deletion set.
    let mut keep = LUID::default();
    let w = wstr("SeChangeNotifyPrivilege");
    unsafe {
        LookupPrivilegeValueW(None, pcwstr(&w), &mut keep)?;
        let _ = CloseHandle(base);
    }
    assert!(
        !to_delete
            .iter()
            .any(|p| p.Luid.LowPart == keep.LowPart && p.Luid.HighPart == keep.HighPart),
        "SeChangeNotifyPrivilege must not be in the deletion set"
    );
    Ok(())
}
