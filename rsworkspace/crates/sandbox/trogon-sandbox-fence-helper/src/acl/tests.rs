use super::*;
use windows::Win32::Security::{GROUP_SECURITY_INFORMATION, GetLengthSid};

fn ace_count(d: &BuiltAcl) -> u16 {
    // ACL.AceCount is bytes 4–5 (LE u16).
    u16::from_le_bytes([d.buf[4], d.buf[5]])
}

/// IsolatedDesk's `[broker, sb, SY]:GA` DACL - 3 ACEs,
/// revision 2, GENERIC_ALL on each. Regression for the
/// `KeptAces`/`rebuild_acl` chokepoint: a sizing or revision
/// bug here breaks the sandbox-user desktop attach (R1 hang).
#[test]
fn isolated_desk_dacl_shape() -> Result<(), Box<dyn std::error::Error>> {
    let d = build_allow_dacl(&[
        Allow("S-1-5-21-1-2-3-1000", Mask::GENERIC_ALL, NO_INHERIT),
        Allow("S-1-5-21-1-2-3-1004", Mask::GENERIC_ALL, NO_INHERIT),
        Allow(SID_SYSTEM, Mask::GENERIC_ALL, NO_INHERIT),
    ])?;
    assert_eq!(d.buf[0], 2, "AclRevision");
    assert_eq!(ace_count(&d), 3);
    Ok(())
}

#[test]
fn build_allow_dacl_dedup() -> Result<(), Box<dyn std::error::Error>> {
    // Same SID twice (case-insensitive) → 1 ACE; first wins.
    let d = build_allow_dacl(&[
        Allow(SID_BUILTIN_ADMINS, Mask::FILE_ALL, NO_INHERIT),
        Allow("s-1-5-32-544", Mask::FILE_READ_EXEC, NO_INHERIT),
    ])?;
    assert_eq!(ace_count(&d), 1);
    Ok(())
}

/// Grant → revoke on a `SE_DACL_PROTECTED` directory must leave
/// the protection bit (and the explicit ACE set) intact.
/// Regression: [`apply_sandbox_aces`] used to write back
/// `UNPROTECTED_` unconditionally, which re-enabled inheritance
/// on a deliberately severed path - durably re-admitting the
/// parent's inheritable ACEs (e.g. a data-drive root's
/// `Authenticated Users:(M)`), and revoke could not undo it.
#[test]
fn recompose_preserves_dacl_protection() -> Result<(), Box<dyn std::error::Error>> {
    // Guests - a resolvable well-known SID that is never the
    // test runner's own identity, standing in for the sandbox
    // user (no account provisioning needed to write ACEs).
    const FAKE_SB_SID: &str = "S-1-5-32-546";
    let dir = std::env::temp_dir().join(format!("fencehelper-prot-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let path = dir.to_str().ok_or("missing value")?;

    // Sever inheritance: protected DACL with an explicit
    // owner-side allow (mirrors `icacls /inheritance:d` plus a
    // lockdown to a fixed ACE set).
    let user_sid = crate::sid::current_user_sid()?;
    let lockdown = build_allow_dacl(&[
        Allow(&user_sid, Mask::FILE_ALL, OICI),
        Allow(SID_SYSTEM, Mask::FILE_ALL, OICI),
    ])?;
    write_file_dacl(path, lockdown.as_ptr(), Protection::Protected)?;
    let (sd, _) = read_file_dacl(path)?;
    assert!(sd_dacl_protected(&sd)?, "setup: DACL protected");

    // Grant + revoke through the production chokepoint.
    let grant = SbAceSet {
        grant: Some(GrantMask::Modify),
        ..Default::default()
    };
    apply_sandbox_aces(path, FAKE_SB_SID, grant)?;
    let (sd, dacl) = read_file_dacl(path)?;
    assert!(sd_dacl_protected(&sd)?, "protection survives grant");
    let (aces, ..) = filter_aces(dacl, |hdr, _| hdr.AceFlags & INHERITED_ACE != 0)?;
    assert!(aces.is_empty(), "no inherited ACEs re-derived after grant");

    apply_sandbox_aces(path, FAKE_SB_SID, SbAceSet::default())?;
    let (sd, dacl) = read_file_dacl(path)?;
    assert!(sd_dacl_protected(&sd)?, "protection survives revoke");
    let (aces, ..) = filter_aces(dacl, |hdr, _| hdr.AceFlags & INHERITED_ACE != 0)?;
    assert!(aces.is_empty(), "no inherited ACEs re-derived after revoke");

    // The common case keeps its contract: an unprotected dir
    // stays unprotected across the same round-trip, and its
    // inherited ACEs (the parent's OICI lockdown entries
    // propagate to it) really are re-derived by the
    // `UNPROTECTED_` write-back - not merely "protection bit
    // still clear".
    let plain = dir.join("plain");
    std::fs::create_dir_all(&plain)?;
    let plain_path = plain.to_str().ok_or("missing value")?;
    apply_sandbox_aces(plain_path, FAKE_SB_SID, grant)?;
    apply_sandbox_aces(plain_path, FAKE_SB_SID, SbAceSet::default())?;
    let (sd, dacl) = read_file_dacl(plain_path)?;
    assert!(!sd_dacl_protected(&sd)?, "unprotected dir stays unprotected");
    let (aces, ..) = filter_aces(dacl, |hdr, _| hdr.AceFlags & INHERITED_ACE != 0)?;
    assert!(!aces.is_empty(), "inherited ACEs re-derived on the unprotected dir");

    std::fs::remove_dir_all(&dir).ok();
    Ok(())
}

const GROUP: &str = "S-1-5-21-1-2-3-1002";
const REAL: &str = "S-1-5-21-1-2-3-1000";
const CAP: &str = "S-1-0-1953719907-1-2-3-4";
const T: PersistentTrustees<'static> = PersistentTrustees {
    group_sid: Some(GROUP),
    real_user_sid: REAL,
    previous_group_sids: &[],
    sandbox_user_sid: None,
    recorded_denies: None,
};

fn root_want<'a>(cap: &'a str) -> PersistentWant<'a> {
    PersistentWant {
        group_sid: GROUP,
        real_user_sid: REAL,
        capability_sid: Some(cap),
        deny: None,
        deny_fdc: false,
        deny_delete: false,
        profile: None,
    }
}

/// The exact ACE list of a persistent root, in DACL order.
#[test]
fn persistent_root_ace_list_is_pinned() {
    let sddl: Vec<String> = persistent_aces(&root_want(CAP)).iter().map(AceView::sddl).collect();
    assert_eq!(
        sddl,
        [
            format!("(A;OICI;0x1301bf;;;{GROUP})"),
            format!("(A;OICI;0x1301bf;;;{CAP})"),
            "(A;OICI;0x20000;;;S-1-3-4)".to_owned(),
            format!("(A;OICI;0x60000;;;{REAL})"),
        ]
    );
    // Without a capability SID there is no ALLOW at all.
    let denies = PersistentWant {
        capability_sid: None,
        deny: Some(DenyMask::WriteDeny),
        deny_fdc: true,
        deny_delete: true,
        ..root_want(CAP)
    };
    let sddl: Vec<String> = persistent_aces(&denies).iter().map(AceView::sddl).collect();
    assert_eq!(
        sddl,
        [
            format!("(D;OICI;{:#x};;;{GROUP})", DenyMask::WriteDeny.bits()),
            format!("(D;;0x10000;;;{GROUP})"),
            format!("(D;OICI;0x40;;;{GROUP})"),
        ]
    );
    let both = persistent_aces(&PersistentWant {
        deny_fdc: true,
        ..root_want(CAP)
    });
    assert_eq!(both.len(), 5);
    assert!(both[0].deny && !both[1..].iter().any(|a| a.deny));
    assert!(
        both.iter()
            .all(|a| [GROUP, CAP, SID_OWNER_RIGHTS, REAL].contains(&a.sid.as_str()))
    );
}

/// What the session-start comparison accepts and what it does not.
#[test]
fn expected_dacl_comparison() {
    let want = persistent_aces(&root_want(CAP));
    let inherited = |sid: &str, mask: u32| AceView {
        deny: false,
        inherited: true,
        flags: 0x3,
        mask,
        sid: sid.to_owned(),
    };
    let other = |sid: &str, mask: u32| AceView {
        inherited: false,
        ..inherited(sid, mask)
    };
    // The user's own full-control ACE and an inherited persistent ACE are not ours.
    let mut dacl = want.clone();
    dacl.push(other(REAL, Mask::FILE_ALL.bits()));
    dacl.push(other(SID_SYSTEM, Mask::FILE_ALL.bits()));
    dacl.push(inherited(GROUP, Mask::MODIFY_NO_FDC.bits()));
    dacl.push(inherited("S-1-0-1953719907-9-9-9-9", Mask::MODIFY_NO_FDC.bits()));
    assert_eq!(persistent_on_disk(&dacl, &T, true), comparable(&want, true));

    let differs = |dacl: &[AceView]| persistent_on_disk(dacl, &T, true) != comparable(&want, true);
    // One ACE missing.
    for i in 0..want.len() {
        let mut d = want.clone();
        d.remove(i);
        assert!(differs(&d), "missing ACE {i}");
    }
    let mut d = want.clone();
    d[0].mask = Mask::FILE_ALL.bits();
    assert!(differs(&d));
    let mut d = want.clone();
    d[1].flags = 0x2;
    assert!(differs(&d));
    let mut d = want.clone();
    d[0].deny = true;
    assert!(differs(&d));
    let mut d = want.clone();
    d[1].sid = "S-1-0-1953719907-9-9-9-9".to_owned();
    assert!(differs(&d));
    let mut d = want.clone();
    d.insert(0, other(GROUP, Mask::FILE_DELETE_CHILD.bits()));
    assert!(differs(&d));
    // Order is part of it.
    let mut d = want.clone();
    d.swap(0, 3);
    assert!(differs(&d));
    // Nothing of ours on disk.
    assert!(persistent_on_disk(&[other(REAL, Mask::FILE_ALL.bits())], &T, true).is_empty());

    // On a file inheritance flags are not compared.
    let mut on_file = want.clone();
    for a in &mut on_file {
        a.flags = 0;
    }
    assert_eq!(persistent_on_disk(&on_file, &T, false), comparable(&want, false));
    assert_ne!(persistent_on_disk(&on_file, &T, true), comparable(&want, true));

    // With the install record gone the group's ACEs are not recognised.
    let no_group = PersistentTrustees { group_sid: None, ..T };
    assert_eq!(persistent_on_disk(&want, &no_group, true).len(), 3);
}

#[test]
fn sid_strings_from_ace_bytes() -> Result<(), Box<dyn std::error::Error>> {
    for sid in [
        "S-1-1-0",
        "S-1-3-4",
        "S-1-5-32-545",
        "S-1-5-21-1-2-3-1000",
        CAP,
        "S-1-0-1953719907-4294967295-0-1-2",
    ] {
        let b = crate::sid::sid_bytes(sid)?;
        assert_eq!(sid_string_from_bytes(&b).as_deref(), Some(sid));
        // A truncated SID is not one.
        assert_eq!(sid_string_from_bytes(&b[..b.len() - 1]), None);
    }
    assert_eq!(sid_string_from_bytes(&[]), None);
    Ok(())
}

#[test]
fn write_checks_on_a_root_dacl() {
    let ace = |sid: &str, mask: u32, flags: u8| AceView {
        deny: false,
        inherited: true,
        flags,
        mask,
        sid: sid.to_owned(),
    };
    let sb = "S-1-5-21-1-2-3-1001";
    assert!(!allows_more_than_read(&[ace(sb, Mask::FILE_READ_EXEC.bits(), 0x3)], sb));
    assert!(allows_more_than_read(&[ace(sb, Mask::MODIFY_NO_FDC.bits(), 0x3)], sb));
    assert!(!allows_more_than_read(
        &[ace(GROUP, Mask::MODIFY_NO_FDC.bits(), 0x3)],
        sb
    ));
    // Ours (OWNER RIGHTS with READ_CONTROL only) is not reported.
    let found = owner_write_paths(&[
        ace(SID_CREATOR_OWNER, Mask::GENERIC_ALL.bits(), 0xb),
        ace(SID_CREATOR_OWNER, Mask::GENERIC_ALL.bits(), 0),
        ace(SID_CREATOR_OWNER, Mask::FILE_READ_EXEC.bits(), 0x3),
        ace(SID_OWNER_RIGHTS, Mask::FILE_ALL.bits(), 0x3),
        ace(SID_OWNER_RIGHTS, Mask::READ_CONTROL.bits(), 0x3),
    ]);
    assert_eq!(found.len(), 2, "{found:?}");
}

/// The persistent ACEs through the real DACL write and read.
#[test]
fn persistent_aces_round_trip_on_disk() -> Result<(), Box<dyn std::error::Error>> {
    // Guests stands in for the sandbox group.
    const GROUP: &str = "S-1-5-32-546";
    let real = crate::sid::current_user_sid()?;
    let cap = crate::persist::new_capability_sid()?;
    let t = PersistentTrustees {
        group_sid: Some(GROUP),
        real_user_sid: &real,
        previous_group_sids: &[],
        sandbox_user_sid: None,
        recorded_denies: None,
    };
    let dir = std::env::temp_dir().join(format!("fencehelper-persist-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("sub"))?;
    std::fs::write(dir.join("sub").join("f.txt"), b"x")?;
    let path = dir.to_str().ok_or("missing value")?;
    let before = read_aces(path)?;

    let want = persistent_aces(&PersistentWant {
        group_sid: GROUP,
        real_user_sid: &real,
        capability_sid: Some(&cap),
        deny: None,
        deny_fdc: true,
        deny_delete: false,
        profile: None,
    });
    for _ in 0..2 {
        write_persistent_aces(path, &t, &want)?;
        let aces = read_aces(path)?;
        assert_eq!(persistent_on_disk(&aces, &t, true), comparable(&want, true));
        assert_eq!(comparable(&aces[..want.len()], true), comparable(&want, true));
    }
    // A file two levels down carries each of them as inherited.
    let file = dir.join("sub").join("f.txt");
    let below = read_aces(file.to_str().ok_or("missing value")?)?;
    for w in &want {
        assert!(
            below
                .iter()
                .any(|a| a.inherited && a.deny == w.deny && a.sid == w.sid && a.mask == w.mask),
            "not inherited: {}",
            w.sddl()
        );
    }
    assert!(persistent_on_disk(&below, &t, false).is_empty());

    write_persistent_aces(path, &t, &[])?;
    let after = read_aces(path)?;
    assert!(persistent_on_disk(&after, &t, true).is_empty());
    let explicit = |v: &[AceView]| v.iter().filter(|a| !a.inherited).cloned().collect::<Vec<_>>();
    assert_eq!(explicit(&after), explicit(&before));
    let below = read_aces(file.to_str().ok_or("missing value")?)?;
    assert!(!below.iter().any(|a| a.sid == cap || a.sid == GROUP));

    std::fs::remove_dir_all(&dir).ok();
    Ok(())
}

/// What the profile read grant writes, per kind of path: one
/// ALLOW for the group, read + execute at most, inheriting only
/// on a directory granted whole.
#[test]
fn profile_ace_lists_are_pinned() {
    let list = |profile: Option<ProfileAce>, deny: Option<DenyMask>, cap: Option<&str>| {
        persistent_aces(&PersistentWant {
            capability_sid: cap,
            deny,
            profile,
            ..root_want(CAP)
        })
        .iter()
        .map(AceView::sddl)
        .collect::<Vec<_>>()
    };
    assert_eq!(
        list(Some(ProfileAce::Container), None, None),
        [format!("(A;;0x1200a9;;;{GROUP})")]
    );
    assert_eq!(
        list(Some(ProfileAce::Granted), None, None),
        [format!("(A;OICI;0x1200a9;;;{GROUP})")]
    );
    assert_eq!(
        list(Some(ProfileAce::File), None, None),
        [format!("(A;;0x1200a9;;;{GROUP})")]
    );
    assert_eq!(
        list(None, Some(DenyMask::ReadDeny), None),
        [format!("(D;OICI;0x1f01ff;;;{GROUP})")]
    );
    assert_eq!(
        list(Some(ProfileAce::Granted), Some(DenyMask::WriteDeny), None),
        [
            format!("(D;OICI;{:#x};;;{GROUP})", DenyMask::WriteDeny.bits()),
            format!("(A;OICI;0x1200a9;;;{GROUP})"),
        ]
    );
    assert_eq!(
        list(Some(ProfileAce::Container), None, Some(CAP)),
        list(None, None, Some(CAP))
    );
    const WRITE_BITS: u32 = 0x2 | 0x4 | 0x10 | 0x40 | 0x100 | 0x1_0000 | 0x4_0000 | 0x8_0000;
    const GENERIC_BITS: u32 = 0xf000_0000;
    for p in [ProfileAce::Container, ProfileAce::Granted, ProfileAce::File] {
        for a in persistent_aces(&PersistentWant {
            capability_sid: None,
            profile: Some(p),
            ..root_want(CAP)
        }) {
            assert!(!a.deny && a.sid == GROUP);
            assert_eq!(a.mask & (WRITE_BITS | GENERIC_BITS), 0, "{p:?}");
        }
    }
    // FILE_GENERIC_READ (list, read EA, read attributes, read
    // control, synchronize) and traverse.
    assert_eq!(Mask::DIR_LIST.bits(), 0x1 | 0x8 | 0x80 | 0x2_0000 | 0x10_0000 | 0x20);
    assert_eq!(Mask::DIR_LIST.bits() & 0x0012_0089, 0x0012_0089);
}

/// A container entry written with the narrower mask of an earlier
/// build is ours, and is not the wanted list: it is rewritten.
#[test]
fn a_container_entry_with_the_earlier_mask_converges() {
    let want = persistent_aces(&PersistentWant {
        capability_sid: None,
        profile: Some(ProfileAce::Container),
        ..root_want(CAP)
    });
    let earlier = AceView {
        mask: 0x0010_00a1,
        ..want[0].clone()
    };
    let listed: [AceView; 0] = [];
    for t in [
        T,
        PersistentTrustees {
            recorded_denies: Some(&listed),
            ..T
        },
    ] {
        assert!(is_persistent_ace(&earlier, &t));
        let on_disk = persistent_on_disk(std::slice::from_ref(&earlier), &t, true);
        assert_eq!(on_disk.len(), 1, "not left as someone else's");
        assert_ne!(on_disk, comparable(&want, true));
        assert_eq!(persistent_on_disk(&want, &t, true), comparable(&want, true));
    }
}

/// An explicit DENY already on the object stays in front of the allows
/// written here; a DACL with allows at the head needs a rewrite.
#[test]
fn existing_denies_stay_ahead_of_the_allows_written_here() -> Result<(), Box<dyn std::error::Error>> {
    const GROUP: &str = "S-1-5-32-546";
    const EVERYONE: &str = "S-1-1-0";
    let real = crate::sid::current_user_sid()?;
    let t = PersistentTrustees {
        group_sid: Some(GROUP),
        real_user_sid: &real,
        previous_group_sids: &[],
        sandbox_user_sid: None,
        recorded_denies: None,
    };
    let cap = crate::persist::new_capability_sid()?;
    let base = std::env::temp_dir().join(format!("fencehelper-canon-{}", std::process::id()));
    let explicit = |p: &str| -> Result<Vec<AceView>, FenceHelperError> {
        Ok(read_aces(p)?.into_iter().filter(|a| !a.inherited).collect())
    };
    let canonical = |aces: &[AceView]| {
        let first_allow = aces.iter().position(|a| !a.deny).unwrap_or(aces.len());
        aces[first_allow..].iter().all(|a| !a.deny)
    };
    let owner_sddl = format!(
        "D:P(D;OICI;0x10040;;;{EVERYONE})(D;OICI;0x1;;;{SID_BUILTIN_USERS})\
         (A;OICI;FA;;;{real})(A;OICI;FA;;;{SID_SYSTEM})"
    );
    for (name, want) in [
        (
            "root",
            persistent_aces(&PersistentWant {
                group_sid: GROUP,
                real_user_sid: &real,
                capability_sid: Some(&cap),
                deny: None,
                deny_fdc: true,
                deny_delete: false,
                profile: None,
            }),
        ),
        (
            "profile",
            persistent_aces(&PersistentWant {
                group_sid: GROUP,
                real_user_sid: &real,
                capability_sid: None,
                deny: None,
                deny_fdc: false,
                deny_delete: false,
                profile: Some(ProfileAce::Granted),
            }),
        ),
    ] {
        let dir = base.join(name);
        std::fs::create_dir_all(&dir)?;
        let path = dir.to_str().ok_or("missing value")?;
        set_path_dacl_from_sddl(path, &owner_sddl, "test")?;
        let before = explicit(path)?;
        write_persistent_aces(path, &t, &want)?;
        let aces = explicit(path)?;
        assert!(canonical(&aces), "{name}: {aces:?}");
        assert!(persistent_layout_ok(&aces, &t), "{name}");
        assert_eq!(persistent_on_disk(&aces, &t, true), comparable(&want, true), "{name}");
        let our_allow = aces
            .iter()
            .position(|a| !a.deny && is_persistent_ace(a, &t))
            .ok_or("missing value")?;
        for foreign in [EVERYONE, SID_BUILTIN_USERS] {
            let at = aces
                .iter()
                .position(|a| a.deny && a.sid == foreign)
                .ok_or("missing value")?;
            assert!(at < our_allow, "{name}: deny for {foreign} behind our allow");
        }
        write_persistent_aces(path, &t, &want)?;
        assert_eq!(explicit(path)?, aces, "{name}");
        write_persistent_aces(path, &t, &[])?;
        assert_eq!(explicit(path)?, before, "{name}");

        let head: String = want.iter().map(AceView::sddl).collect();
        let old_sddl = format!("D:P{head}{}", &owner_sddl[3..]);
        set_path_dacl_from_sddl(path, &old_sddl, "test")?;
        let old = explicit(path)?;
        assert_eq!(
            persistent_on_disk(&old, &t, true),
            comparable(&want, true),
            "{name}: the old layout holds the same entries"
        );
        let has_our_allow_first = want.iter().any(|a| !a.deny);
        assert_eq!(
            persistent_layout_ok(&old, &t),
            !has_our_allow_first,
            "{name}: the old layout is not canonical"
        );
        write_persistent_aces(path, &t, &want)?;
        let repaired = explicit(path)?;
        assert!(canonical(&repaired), "{name}: {repaired:?}");
        assert!(persistent_layout_ok(&repaired, &t), "{name}");
        set_path_dacl_from_sddl(path, &format!("D:P(A;OICI;FA;;;{real})"), "test")?;
    }
    std::fs::remove_dir_all(&base).ok();
    Ok(())
}

/// Entries that are not ours are never moved by either writer, and taking
/// ours out leaves the list exactly as it was, canonical or not.
#[test]
fn foreign_entries_keep_their_order_and_removal_restores_the_list() -> Result<(), Box<dyn std::error::Error>> {
    const GROUP: &str = "S-1-5-32-546";
    const SB: &str = "S-1-5-32-547";
    const EVERYONE: &str = "S-1-1-0";
    let real = crate::sid::current_user_sid()?;
    let t = PersistentTrustees {
        group_sid: Some(GROUP),
        real_user_sid: &real,
        previous_group_sids: &[],
        sandbox_user_sid: None,
        recorded_denies: None,
    };
    let cap = crate::persist::new_capability_sid()?;
    let root = persistent_aces(&PersistentWant {
        group_sid: GROUP,
        real_user_sid: &real,
        capability_sid: Some(&cap),
        deny: None,
        deny_fdc: true,
        deny_delete: false,
        profile: None,
    });
    let session = SbAceSet {
        grant: Some(GrantMask::ReadOnly),
        deny_fdc: true,
        ..Default::default()
    };
    let explicit = |p: &str| -> Result<Vec<AceView>, FenceHelperError> {
        Ok(read_aces(p)?.into_iter().filter(|a| !a.inherited).collect())
    };
    let sddl = |aces: &[AceView]| -> String { aces.iter().map(AceView::sddl).collect() };
    let ours = |a: &AceView| is_persistent_ace(a, &t) || a.sid == SB;
    let base = std::env::temp_dir().join(format!("fencehelper-place-{}", std::process::id()));
    for (name, layout) in [
        (
            "posix",
            format!(
                "(A;OICI;FA;;;{real})(D;OICI;0x116;;;{SID_BUILTIN_USERS})\
                 (A;OICI;0x1200a9;;;{SID_BUILTIN_USERS})"
            ),
        ),
        (
            "denies-first",
            format!(
                "(D;OICI;0x10040;;;{EVERYONE})(D;OICI;0x2;;;{SID_BUILTIN_USERS})\
                 (A;OICI;FA;;;{real})(A;OICI;FA;;;{SID_SYSTEM})"
            ),
        ),
        (
            "mixed",
            format!(
                "(A;OICI;FA;;;{real})(D;OICI;0x2;;;{EVERYONE})\
                 (A;OICI;0x1200a9;;;{SID_BUILTIN_USERS})(D;OICI;0x10000;;;{SID_BUILTIN_USERS})\
                 (A;OICI;FA;;;{SID_SYSTEM})"
            ),
        ),
        ("allows-only", format!("(A;OICI;FA;;;{real})(A;OICI;FA;;;{SID_SYSTEM})")),
    ] {
        let dir = base.join(name);
        std::fs::create_dir_all(&dir)?;
        let path = dir.to_str().ok_or("missing value")?;
        set_path_dacl_from_sddl(path, &format!("D:P{layout}"), "test")?;
        let before = explicit(path)?;
        assert_eq!(before.len(), layout.matches('(').count(), "{name}: setup");
        let last_foreign_deny = before.iter().rposition(|a| a.deny);

        let check = |when: &str| -> Result<_, Box<dyn std::error::Error>> {
            let aces = explicit(path)?;
            let foreign: Vec<AceView> = aces.iter().filter(|a| !ours(a)).cloned().collect();
            assert_eq!(foreign, before, "{name}, {when}: the others moved");
            let first_allow = aces.iter().position(|a| !a.deny).ok_or("no allow ACE")?;
            for (i, a) in aces.iter().enumerate() {
                if a.deny && ours(a) {
                    assert!(i < first_allow, "{name}, {when}: {a:?} behind an allow");
                }
            }
            let first_ours = aces.iter().position(|a| !a.deny && ours(a));
            if let Some(first_ours) = first_ours {
                assert!(
                    aces[first_ours..].iter().all(|a| !a.deny),
                    "{name}, {when}: a deny behind our allow: {}",
                    sddl(&aces)
                );
                let foreign_before = aces[..first_ours].iter().filter(|a| !ours(a)).count();
                assert_eq!(
                    foreign_before,
                    last_foreign_deny.map_or(0, |i| i + 1),
                    "{name}, {when}: {}",
                    sddl(&aces)
                );
            }
            assert!(persistent_layout_ok(&aces, &t), "{name}, {when}");
            Ok(aces)
        };

        write_persistent_aces(path, &t, &root)?;
        let aces = check("kept grant")?;
        assert_eq!(persistent_on_disk(&aces, &t, true), comparable(&root, true));
        write_persistent_aces(path, &t, &root)?;
        assert_eq!(explicit(path)?, aces, "{name}: a second write changes nothing");
        write_persistent_aces(path, &t, &[])?;
        assert_eq!(explicit(path)?, before, "{name}: kept grant removed");

        apply_sandbox_aces(path, SB, session)?;
        check("session grant")?;
        apply_sandbox_aces(path, SB, SbAceSet::default())?;
        assert_eq!(explicit(path)?, before, "{name}: session grant removed");

        write_persistent_aces(path, &t, &root)?;
        apply_sandbox_aces(path, SB, session)?;
        let both = check("kept, then session")?;
        assert_eq!(persistent_on_disk(&both, &t, true), comparable(&root, true));
        write_persistent_aces(path, &t, &root)?;
        check("kept rewritten under the session")?;
        apply_sandbox_aces(path, SB, SbAceSet::default())?;
        check("session gone, kept stays")?;
        apply_sandbox_aces(path, SB, session)?;
        write_persistent_aces(path, &t, &[])?;
        check("kept gone, session stays")?;
        apply_sandbox_aces(path, SB, SbAceSet::default())?;
        assert_eq!(explicit(path)?, before, "{name}: both removed");

        set_path_dacl_from_sddl(path, &format!("D:P(A;OICI;FA;;;{real})"), "test")?;
    }
    std::fs::remove_dir_all(&base).ok();
    Ok(())
}

/// A session allow admitted on an excluded path goes ahead of the
/// group's exclusion deny on that object - and only there.
#[test]
fn an_admitted_session_allow_precedes_our_own_exclusion_deny() -> Result<(), Box<dyn std::error::Error>> {
    const GROUP: &str = "S-1-5-32-546";
    const SB: &str = "S-1-5-32-547";
    const EVERYONE: &str = "S-1-1-0";
    let real = crate::sid::current_user_sid()?;
    let t = PersistentTrustees {
        group_sid: Some(GROUP),
        real_user_sid: &real,
        previous_group_sids: &[],
        sandbox_user_sid: Some(SB),
        recorded_denies: None,
    };
    let exclusion = persistent_aces(&PersistentWant {
        group_sid: GROUP,
        real_user_sid: &real,
        capability_sid: None,
        deny: Some(DenyMask::ReadDeny),
        deny_fdc: false,
        deny_delete: false,
        profile: None,
    });
    let read = SbAceSet {
        grant: Some(GrantMask::ReadOnly),
        ..Default::default()
    };
    let explicit = |p: &str| -> Result<Vec<AceView>, FenceHelperError> {
        Ok(read_aces(p)?.into_iter().filter(|a| !a.inherited).collect())
    };
    let at = |aces: &[AceView], sid: &str, deny: bool| aces.iter().position(|a| a.sid == sid && a.deny == deny);
    let base = std::env::temp_dir().join(format!("fencehelper-admit-{}", std::process::id()));
    let dir = base.join("store");
    std::fs::create_dir_all(dir.join("inner"))?;
    let path = dir.to_str().ok_or("missing value")?;
    set_path_dacl_from_sddl(
        path,
        &format!("D:P(D;OICI;0x10000;;;{EVERYONE})(A;OICI;FA;;;{real})"),
        "test",
    )?;
    write_persistent_aces(path, &t, &exclusion)?;
    let before = explicit(path)?;

    apply_sandbox_aces_over(path, SB, read, Some(GROUP))?;
    let aces = explicit(path)?;
    assert!(
        at(&aces, SB, false) < at(&aces, GROUP, true),
        "the allow is behind the exclusion deny: {aces:?}"
    );
    assert!(persistent_layout_ok(&aces, &t));
    assert_eq!(persistent_on_disk(&aces, &t, true), comparable(&exclusion, true));
    write_persistent_aces(path, &t, &exclusion)?;
    let rewritten = explicit(path)?;
    assert!(at(&rewritten, GROUP, true) < at(&rewritten, SB, false), "{rewritten:?}");
    assert_eq!(at(&rewritten, GROUP, true), Some(0));
    apply_sandbox_aces_over(path, SB, read, Some(GROUP))?;
    assert_eq!(explicit(path)?, aces);
    apply_sandbox_aces_over(path, SB, SbAceSet::default(), Some(GROUP))?;
    assert_eq!(explicit(path)?, before);

    let inner = dir.join("inner");
    let inner = inner.to_str().ok_or("missing value")?;
    apply_sandbox_aces_over(inner, SB, read, Some(GROUP))?;
    let all = read_aces(inner)?;
    let allow = all.iter().position(|a| a.sid == SB && !a.deny).ok_or("missing value")?;
    let inherited = all
        .iter()
        .position(|a| a.sid == GROUP && a.deny)
        .ok_or("missing value")?;
    assert!(allow < inherited && all[inherited].inherited);
    apply_sandbox_aces_over(inner, SB, SbAceSet::default(), Some(GROUP))?;

    let both = SbAceSet { deny_fdc: true, ..read };
    apply_sandbox_aces_over(path, SB, both, Some(GROUP))?;
    let aces = explicit(path)?;
    assert!(at(&aces, SB, true) < at(&aces, SB, false));
    assert!(at(&aces, SB, false) < at(&aces, GROUP, true));
    apply_sandbox_aces_over(path, SB, SbAceSet::default(), Some(GROUP))?;

    write_persistent_aces(path, &t, &[])?;
    for group in [None, Some(GROUP)] {
        apply_sandbox_aces_over(path, SB, read, group)?;
        let aces = explicit(path)?;
        assert!(at(&aces, EVERYONE, true) < at(&aces, SB, false), "{aces:?}");
        apply_sandbox_aces_over(path, SB, SbAceSet::default(), group)?;
    }
    let fdc_only = persistent_aces(&PersistentWant {
        group_sid: GROUP,
        real_user_sid: &real,
        capability_sid: None,
        deny: None,
        deny_fdc: true,
        deny_delete: false,
        profile: None,
    });
    write_persistent_aces(path, &t, &fdc_only)?;
    apply_sandbox_aces_over(path, SB, read, Some(GROUP))?;
    let aces = explicit(path)?;
    assert!(at(&aces, EVERYONE, true) < at(&aces, SB, false), "{aces:?}");

    set_path_dacl_from_sddl(path, &format!("D:P(A;OICI;FA;;;{real})"), "test")?;
    std::fs::remove_dir_all(&base).ok();
    Ok(())
}

/// Entries naming the group of an earlier install are recognised
/// as ours when the marker still lists that group.
#[test]
fn entries_of_a_previous_group_are_ours_to_remove() {
    let old = "S-1-5-21-1-2-3-1001".to_owned();
    let ace = |sid: &str, deny: bool| AceView {
        deny,
        sid: sid.to_owned(),
        mask: 0x1200a9,
        flags: 3,
        inherited: false,
    };
    let previous = [old.clone()];
    let with = PersistentTrustees {
        group_sid: Some("S-1-5-21-1-2-3-2002"),
        real_user_sid: "S-1-5-21-9-9-9-500",
        previous_group_sids: &previous,
        sandbox_user_sid: None,
        recorded_denies: None,
    };
    let without = PersistentTrustees {
        previous_group_sids: &[],
        sandbox_user_sid: None,
        ..with
    };
    for deny in [false, true] {
        assert!(is_persistent_ace(&ace(&old, deny), &with));
        assert!(!is_persistent_ace(&ace(&old, deny), &without));
        assert_eq!(is_persistent_ace(&ace("S-1-5-21-1-2-3-2002", deny), &with), !deny);
        assert!(!is_persistent_ace(&ace("S-1-5-32-545", deny), &with));
    }
}

/// The fence an embedder may put on a write root for the sandbox
/// group: do not delete, rename or re-permission this folder.
const FENCE: u32 = 0x0001_0000 | 0x0004_0000 | 0x0008_0000 | 0x40;

/// Which entries for the sandbox group are the product's.
#[test]
fn a_group_deny_the_record_does_not_list_is_foreign() {
    let deny = |sid: &str, mask: u32, flags: u8| AceView {
        deny: true,
        inherited: false,
        flags,
        mask,
        sid: sid.to_owned(),
    };
    let allow = |sid: &str, mask: u32| AceView {
        deny: false,
        ..deny(sid, mask, 0x3)
    };
    let fence = deny(GROUP, FENCE, 0);
    let want = persistent_aces(&PersistentWant {
        deny: Some(DenyMask::WriteDeny),
        deny_fdc: true,
        deny_delete: true,
        ..root_want(CAP)
    });
    let listed: Vec<AceView> = want.iter().filter(|a| a.deny).cloned().collect();
    let with_record = PersistentTrustees {
        recorded_denies: Some(&listed),
        ..T
    };
    for t in [&T, &with_record] {
        assert!(!is_persistent_ace(&fence, t));
        assert!(!is_persistent_ace(&deny(CAP, FENCE, 0), t));
        for w in &listed {
            assert!(is_persistent_ace(w, t), "{}", w.sddl());
        }
        let exclusion = persistent_aces(&PersistentWant {
            capability_sid: None,
            deny: Some(DenyMask::ReadDeny),
            ..root_want(CAP)
        });
        let t_ex = PersistentTrustees {
            recorded_denies: t.recorded_denies.map(|_| exclusion.as_slice()),
            ..*t
        };
        assert!(is_persistent_ace(&exclusion[0], &t_ex));
        assert!(is_persistent_ace(&allow(GROUP, 0x1f01ff), t));
        assert!(is_persistent_ace(&allow(CAP, 0x1200a9), t));
        assert!(!is_persistent_ace(&deny(SID_BUILTIN_USERS, FENCE, 0), t));
    }
    let delete_only = deny(GROUP, Mask::DELETE.bits(), 0);
    let none_listed = PersistentTrustees {
        recorded_denies: Some(&[]),
        ..T
    };
    assert!(is_persistent_ace(&delete_only, &T));
    assert!(!is_persistent_ace(&delete_only, &none_listed));
    assert!(!product_deny_shape(FENCE));

    let want_c = comparable(&want, true);
    let mut dacl = vec![fence.clone()];
    dacl.extend(want.clone());
    assert_eq!(persistent_on_disk(&dacl, &with_record, true), want_c);
    assert!(persistent_layout_ok(&dacl, &with_record));
    assert_eq!(foreign_group_denies(&dacl, &with_record), std::slice::from_ref(&fence));
    let mut stray = dacl.clone();
    stray.push(allow(GROUP, 0x1f01ff));
    assert_ne!(persistent_on_disk(&stray, &with_record, true), want_c);
    let mut behind = want.clone();
    behind.push(fence.clone());
    assert_eq!(persistent_on_disk(&behind, &with_record, true), want_c);
    assert!(!persistent_layout_ok(&behind, &with_record));
}

/// The one test for writing without the walk of the tree.
#[test]
fn only_a_change_to_what_is_handed_down_needs_the_walk() {
    let ace = |ty: u8, flags: u8, tag: u8| vec![ty, flags, 12, 0, tag, 0, 0, 0, 1, 1, 0, 0];
    const OI: u8 = 0x1;
    const CI: u8 = 0x2;
    const NP: u8 = 0x4;
    const IO: u8 = 0x8;
    let a = ace(0, OI | CI, 1);
    let b = ace(1, CI, 2);
    let own = ace(0, 0, 3);
    let base = vec![a.clone(), own.clone(), b.clone()];
    let same = |after: &[Vec<u8>]| same_inheritable(&base, after);
    assert!(same(&[a.clone(), b.clone()]));
    assert!(same(&[ace(1, 0, 9), a.clone(), ace(0, NP, 8), b.clone(), own.clone()]));
    assert!(same(&[a.clone(), ace(0, 0, 77), b.clone()]));
    assert!(same_inheritable(std::slice::from_ref(&own), &[]));
    assert!(same_inheritable(&[], &[]));
    assert!(!same(&[a.clone(), own.clone()]));
    assert!(!same(&[b.clone(), own.clone(), a.clone()]));
    assert!(!same(&[a.clone(), own.clone(), ace(1, CI, 3)]));
    assert!(!same(&[a.clone(), own.clone(), ace(0, CI, 2)]));
    for flags in [OI, CI, OI | CI, OI | IO, CI | IO, OI | CI | IO, CI | NP, OI | NP] {
        let extra = ace(0, flags, 5);
        assert!(!same(&[a.clone(), own.clone(), b.clone(), extra]), "{flags:#x}");
    }
    assert!(!same(&[ace(0, OI, 1), own.clone(), b.clone()]));
    assert!(!same(&[ace(0, OI | CI | IO, 1), own.clone(), b.clone()]));
    assert!(!same(&[ace(0, 0, 1), own.clone(), b.clone()]));
    assert!(!same(&[a.clone(), ace(0, OI, 3), b.clone()]));
}

/// A write that changes only an entry of the directory itself touches
/// nothing below; one that changes what is handed down reaches it. A
/// foreign group deny survives both; a stray group allow does not.
#[test]
fn a_write_walks_the_tree_only_when_what_is_handed_down_changes() -> Result<(), Box<dyn std::error::Error>> {
    const GROUP: &str = "S-1-5-32-546";
    let real = crate::sid::current_user_sid()?;
    let cap = crate::persist::new_capability_sid()?;
    let dir = std::env::temp_dir().join(format!("fencehelper-local-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub").join("deep"))?;
    std::fs::write(dir.join("sub").join("deep").join("f.txt"), b"x")?;
    let path = dir.to_str().ok_or("missing value")?;
    let child = dir.join("sub");
    let child = child.to_str().ok_or("missing value")?;
    let grandchild = dir.join("sub").join("deep").join("f.txt");
    let grandchild = grandchild.to_str().ok_or("missing value")?;
    let state = |p: &str| -> Result<_, Box<dyn std::error::Error>> {
        let (sd, _) = read_file_dacl(p)?;
        Ok((read_aces(p)?, sd_dacl_auto_inherited(&sd)?, sd_dacl_protected(&sd)?))
    };
    let below = || -> Result<_, Box<dyn std::error::Error>> { Ok((state(child)?, state(grandchild)?)) };
    let inherited_raw = |p: &str| -> Result<_, Box<dyn std::error::Error>> {
        let (_sd, dacl) = read_file_dacl(p)?;
        let mut out: Vec<Vec<u8>> = Vec::new();
        filter_aces(dacl, |hdr, body| {
            if hdr.AceFlags & INHERITED_ACE != 0 {
                out.push(body.to_vec());
            }
            false
        })?;
        Ok(out)
    };
    let owner_and_group = |p: &str| -> Result<_, Box<dyn std::error::Error>> {
        let w = wstr(p);
        let (mut owner, mut group) = (PSID::default(), PSID::default());
        let mut psd = PSECURITY_DESCRIPTOR::default();
        let r = unsafe {
            GetNamedSecurityInfoW(
                pcwstr(&w),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION,
                Some(&mut owner),
                Some(&mut group),
                None,
                None,
                &mut psd,
            )
        };
        win32_ok(r, "GetNamedSecurityInfoW(owner, group)")?;
        let _sd = OwnedSd::from_raw(psd);
        let text = |sid: PSID| -> Result<_, Box<dyn std::error::Error>> {
            let len = unsafe { GetLengthSid(sid) } as usize;
            let bytes = unsafe { std::slice::from_raw_parts(sid.0 as *const u8, len) };
            Ok(sid_string_from_bytes(bytes).ok_or("malformed SID")?)
        };
        Ok((text(owner)?, text(group)?))
    };
    let counted = |f: &dyn Fn() -> Result<Written, FenceHelperError>| -> Result<_, Box<dyn std::error::Error>> {
        let before = write_counts();
        let how = f()?;
        let after = write_counts();
        Ok((how, after.0 - before.0, after.1 - before.1))
    };

    let root_marks = state(path)?;
    let root_inherited = inherited_raw(path)?;
    let root_owner = owner_and_group(path)?;
    assert!(!root_inherited.is_empty(), "setup: the root inherits");
    let fence = AceView {
        deny: true,
        inherited: false,
        flags: 0,
        mask: FENCE,
        sid: GROUP.to_owned(),
    };
    let (sd, old) = read_file_dacl(path)?;
    let group = LocalPsid::from_string(GROUP)?;
    let laid = place_around(
        old,
        |h, _| h.AceFlags & INHERITED_ACE == 0,
        Placement::default(),
        &[NewAce::Deny(group.as_psid(), FENCE, NO_INHERIT)],
        &[],
    )?;
    let untouched = below()?;
    assert_eq!(
        commit_dacl(DaclTarget::Path(path), &sd, old, &laid, Commit::Write)?,
        Written::Local
    );
    drop(sd);
    assert_eq!(below()?, untouched, "an object-only write reached below");
    let now = state(path)?;
    assert_eq!(
        (now.1, now.2),
        (root_marks.1, root_marks.2),
        "the root's auto-inherited mark and protection"
    );
    assert_eq!(inherited_raw(path)?, root_inherited);
    assert_eq!(owner_and_group(path)?, root_owner);

    let want = persistent_aces(&PersistentWant {
        group_sid: GROUP,
        real_user_sid: &real,
        capability_sid: Some(&cap),
        deny: None,
        deny_fdc: false,
        deny_delete: false,
        profile: None,
    });
    let t = PersistentTrustees {
        group_sid: Some(GROUP),
        real_user_sid: &real,
        previous_group_sids: &[],
        sandbox_user_sid: None,
        recorded_denies: Some(&[]),
    };
    let (how, walks, _) = counted(&|| write_persistent_aces(path, &t, &want))?;
    assert_eq!((how, walks), (Written::Tree, 1));
    let aces = read_aces(path)?;
    assert_eq!(persistent_on_disk(&aces, &t, true), comparable(&want, true));
    assert!(persistent_layout_ok(&aces, &t));
    assert_eq!(foreign_group_denies(&aces, &t), std::slice::from_ref(&fence));
    let at = |aces: &[AceView], deny: bool| aces.iter().position(|a| a.sid == GROUP && a.deny == deny);
    assert!(at(&aces, true) < at(&aces, false));
    assert!(
        state(grandchild)?.0.iter().any(|a| a.inherited && a.sid == cap),
        "the grant did not reach a deep child"
    );
    let granted_below = below()?;
    let marked = state(path)?;
    assert!(marked.1, "setup: the walking write marks the list");

    let fenced = persistent_aces(&PersistentWant {
        group_sid: GROUP,
        real_user_sid: &real,
        capability_sid: Some(&cap),
        deny: None,
        deny_fdc: false,
        deny_delete: true,
        profile: None,
    });
    let listed: Vec<AceView> = fenced.iter().filter(|a| a.deny).cloned().collect();
    let t_fenced = PersistentTrustees {
        recorded_denies: Some(&listed),
        ..t
    };
    let (how, walks, local) = counted(&|| write_persistent_aces(path, &t_fenced, &fenced))?;
    assert_eq!((how, walks, local), (Written::Local, 0, 1));
    assert_eq!(below()?, granted_below);
    let aces = read_aces(path)?;
    assert_eq!(persistent_on_disk(&aces, &t_fenced, true), comparable(&fenced, true));
    assert_eq!(foreign_group_denies(&aces, &t_fenced), std::slice::from_ref(&fence));
    let now = state(path)?;
    assert_eq!(
        (now.1, now.2),
        (true, marked.2),
        "the auto-inherited mark was lost, or protection changed"
    );
    assert_eq!(inherited_raw(path)?, root_inherited);
    assert_eq!(owner_and_group(path)?, root_owner);
    let before = write_counts();
    assert_eq!(
        write_persistent_aces_at(DaclTarget::Path(path), &t_fenced, &fenced, true)?,
        Written::Tree
    );
    assert_eq!(write_counts().0 - before.0, 1);

    let (sd, old) = read_file_dacl(path)?;
    let with_stray = place_around(
        old,
        |h, _| h.AceFlags & INHERITED_ACE == 0,
        Placement::default(),
        &[],
        &[NewAce::Allow(group.as_psid(), Mask::FILE_ALL.bits(), NO_INHERIT)],
    )?;
    commit_dacl(DaclTarget::Path(path), &sd, old, &with_stray, Commit::Write)?;
    drop(sd);
    let aces = read_aces(path)?;
    assert_ne!(persistent_on_disk(&aces, &t_fenced, true), comparable(&fenced, true));
    write_persistent_aces(path, &t_fenced, &fenced)?;
    let aces = read_aces(path)?;
    assert_eq!(persistent_on_disk(&aces, &t_fenced, true), comparable(&fenced, true));
    assert!(
        !aces
            .iter()
            .any(|a| !a.deny && a.sid == GROUP && a.mask == Mask::FILE_ALL.bits())
    );

    let (how, walks, _) = counted(&|| write_persistent_aces(path, &t_fenced, &[]))?;
    assert_eq!((how, walks), (Written::Tree, 1));
    let aces = read_aces(path)?;
    assert!(persistent_on_disk(&aces, &t_fenced, true).is_empty());
    assert_eq!(foreign_group_denies(&aces, &t_fenced), std::slice::from_ref(&fence));
    assert!(!state(grandchild)?.0.iter().any(|a| a.sid == cap));
    type Seen = (Vec<AceView>, bool, bool);
    type Kept<'a> = (&'a [AceView], bool);
    fn lists(s: &(Seen, Seen)) -> (Kept<'_>, Kept<'_>) {
        ((&s.0.0, s.0.2), (&s.1.0, s.1.2))
    }
    assert_eq!(lists(&below()?), lists(&untouched), "forgetting restores what is below");

    let session = SbAceSet {
        deny_delete: true,
        ..Default::default()
    };
    const SB: &str = "S-1-5-32-547";
    let before = write_counts();
    apply_sandbox_aces_over(path, SB, session, None)?;
    assert_eq!((write_counts().0 - before.0, write_counts().1 - before.1), (0, 1));
    let before = write_counts();
    apply_sandbox_aces_over(path, SB, session, None)?;
    assert_eq!(write_counts(), before, "an unchanged list is not written");
    apply_sandbox_aces_over(path, SB, SbAceSet::default(), None)?;
    assert_eq!(lists(&below()?), lists(&untouched));
    let shut = dir.join("shut");
    std::fs::create_dir_all(&shut)?;
    let shut = shut.to_str().ok_or("missing value")?;
    let lockdown = build_allow_dacl(&[
        Allow(&real, Mask::FILE_ALL, OICI),
        Allow(SID_SYSTEM, Mask::FILE_ALL, OICI),
    ])?;
    write_file_dacl(shut, lockdown.as_ptr(), Protection::Protected)?;
    let shut_before = state(shut)?;
    assert!(shut_before.2, "setup: protected");
    let (sd, old) = read_file_dacl(shut)?;
    let with_own = place_around(
        old,
        |h, _| h.AceFlags & INHERITED_ACE == 0,
        Placement::default(),
        &[NewAce::Deny(group.as_psid(), FENCE, NO_INHERIT)],
        &[],
    )?;
    assert_eq!(
        commit_dacl(DaclTarget::Path(shut), &sd, old, &with_own, Commit::Write)?,
        Written::Local
    );
    drop(sd);
    let shut_after = state(shut)?;
    assert_eq!((shut_after.1, shut_after.2), (shut_before.1, true));
    assert_eq!(shut_after.0.len(), shut_before.0.len() + 1);
    assert!(inherited_raw(shut)?.is_empty());
    assert_eq!(owner_and_group(shut)?, owner_and_group(path)?);

    let before = write_counts();
    apply_sandbox_aces(path, SB, SbAceSet::default())?;
    assert_eq!(write_counts().0 - before.0, 1);

    std::fs::remove_dir_all(&dir).ok();
    Ok(())
}

/// A session's deny for the sandbox user stays in front of the
/// group's allow, whichever of the two is written last, and
/// removing the group's ACEs leaves it.
#[test]
fn a_session_deny_stays_ahead_of_the_group_allow() -> Result<(), Box<dyn std::error::Error>> {
    const GROUP: &str = "S-1-5-32-546";
    const SB: &str = "S-1-5-32-547";
    let real = crate::sid::current_user_sid()?;
    let t = PersistentTrustees {
        group_sid: Some(GROUP),
        real_user_sid: &real,
        previous_group_sids: &[],
        sandbox_user_sid: None,
        recorded_denies: None,
    };
    let want = persistent_aces(&PersistentWant {
        group_sid: GROUP,
        real_user_sid: &real,
        capability_sid: None,
        deny: None,
        deny_fdc: false,
        deny_delete: false,
        profile: Some(ProfileAce::Granted),
    });
    let deny = SbAceSet {
        deny: Some(DenyMask::ReadDeny),
        ..Default::default()
    };
    let check = |path: &str, when: &str| -> Result<_, Box<dyn std::error::Error>> {
        let aces: Vec<AceView> = read_aces(path)?.into_iter().filter(|a| !a.inherited).collect();
        let d = aces.iter().position(|a| a.deny && a.sid == SB);
        let a = aces.iter().position(|a| !a.deny && a.sid == GROUP);
        assert!(matches!((d, a), (Some(d), Some(a)) if d < a), "{when}: {aces:?}");
        assert_eq!(persistent_on_disk(&aces, &t, true), comparable(&want, true), "{when}");
        Ok(())
    };
    let base = std::env::temp_dir().join(format!("fencehelper-order-{}", std::process::id()));
    let a = base.join("deny-first");
    std::fs::create_dir_all(&a)?;
    let a = a.to_str().ok_or("missing value")?;
    apply_sandbox_aces(a, SB, deny)?;
    write_persistent_aces(a, &t, &want)?;
    check(a, "allow written after the deny")?;
    let b = base.join("allow-first");
    std::fs::create_dir_all(&b)?;
    let b = b.to_str().ok_or("missing value")?;
    write_persistent_aces(b, &t, &want)?;
    apply_sandbox_aces(b, SB, deny)?;
    check(b, "deny written after the allow")?;
    write_persistent_aces(b, &t, &want)?;
    check(b, "allow rewritten")?;
    write_persistent_aces(b, &t, &[])?;
    let left = read_aces(b)?;
    assert!(left.iter().any(|a| a.deny && !a.inherited && a.sid == SB));
    assert!(!left.iter().any(|a| a.sid == GROUP));
    std::fs::remove_dir_all(&base).ok();
    Ok(())
}

#[test]
fn mask_consts_regression() {
    let m = Mask::MODIFY_NO_FDC;
    assert_eq!(m.bits(), 0x0013_01bf);
    assert_ne!(m.bits() & Mask::DELETE.bits(), 0, "must carry DELETE");
    assert_eq!(m.bits() & Mask::FILE_DELETE_CHILD.bits(), 0);
    assert_eq!(m.bits() & 0xffe0_0000, 0, "stray high bits");

    let ow = Allow::OWNER_RIGHTS.1;
    assert_eq!(ow.bits(), Mask::READ_CONTROL.bits());
    assert_ne!(ow.bits(), 0);
    assert_eq!(ow.bits() & Mask::WRITE_DAC.bits(), 0);
}
