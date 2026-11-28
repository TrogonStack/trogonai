use super::*;
use crate::error::FenceHelperError;
use AtPath::{Another, Nothing, Recorded, Unavailable};
use std::cell::RefCell;

/// A strip that was begun stays on record when its transaction is
/// rolled back, and such a record does not vouch for the tree.
#[test]
fn a_strip_that_was_begun_is_unfinished_until_it_completes() -> Result<(), Box<dyn std::error::Error>> {
    let conn = crate::state_db::open_memory_db_for_tests()?;
    let (root, target, other) = (r"\\?\C:\w", r"\\?\C:\w\deny.txt", r"\\?\C:\other");
    let id = [7u8; 24];
    for p in [root, target, other] {
        conn.execute(
            "INSERT INTO persistent_writes (canonical_path, file_id, spec, done) \
             VALUES (?1, ?2, 'spec', 1)",
            params![p, id.as_slice()],
        )?;
    }
    conn.execute(
        "INSERT INTO persistent_denies (canonical_path, root, kind, mask, file_id) \
         VALUES (?1, ?2, 'deny', 'denyWrite', ?3)",
        params![target, root, id.as_slice()],
    )?;
    let done = |p: &str| -> rusqlite::Result<i64> {
        conn.query_row(
            "SELECT done FROM persistent_writes WHERE canonical_path = ?1",
            params![p],
            |r| r.get(0),
        )
    };
    mark_strip_begun(&conn, root)?;
    // What the forget does next is undone when the process dies.
    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM persistent_denies", [])?;
    tx.execute("UPDATE persistent_writes SET done = 1", [])?;
    drop(tx);
    assert_eq!((done(root)?, done(target)?, done(other)?), (0, 0, 1));
    // Nothing wanted and nothing of ours on the object any more is
    // settled only if no strip is outstanding.
    let rec = |d: i64| (id.to_vec(), "spec".to_owned(), d);
    assert!(!is_settled(Some(&rec(0)), true, "", &id));
    assert!(is_settled(Some(&rec(1)), true, "", &id));
    assert!(is_settled(None, true, "", &id));
    // With ACEs wanted: a completed write of that list on that object.
    assert!(is_settled(Some(&rec(1)), false, "spec", &id));
    assert!(!is_settled(Some(&rec(1)), false, "other", &id));
    assert!(!is_settled(Some(&rec(1)), false, "spec", &[8u8; 24]));
    assert!(!is_settled(None, false, "spec", &id));
    Ok(())
}

/// The denies a write record lists are read back from its text:
/// what tells a deny of ours from someone else's on the same
/// path.
#[test]
fn the_denies_of_a_write_record_are_read_back() -> Result<(), Box<dyn std::error::Error>> {
    const GROUP: &str = "S-1-5-21-1-2-3-1002";
    const CAP: &str = "S-1-0-1953719907-1-2-3-4";
    let want = acl::persistent_aces(&PersistentWant {
        group_sid: GROUP,
        real_user_sid: "S-1-5-21-1-2-3-1000",
        capability_sid: Some(CAP),
        deny: Some(acl::DenyMask::WriteDeny),
        deny_fdc: true,
        deny_delete: true,
        profile: None,
    });
    let spec = spec_of(&want);
    let key = |v: &[AceView]| -> Vec<(String, u32)> { v.iter().map(|a| (a.sid.clone(), a.mask)).collect() };
    let wanted_denies: Vec<AceView> = want.iter().filter(|a| a.deny).cloned().collect();
    assert_eq!(wanted_denies.len(), 3);
    assert_eq!(key(&denies_of_spec(&spec)), key(&wanted_denies));
    assert!(denies_of_spec("").is_empty());
    assert!(denies_of_spec("(A;OICI;0x1301bf;;;S-1-1-0)(nonsense").is_empty());
    assert!(recorded_denies(&want, None).is_none());
    let now: Vec<AceView> = want.iter().filter(|a| !a.deny).cloned().collect();
    let listed = recorded_denies(&now, Some(&spec)).ok_or("missing value")?;
    assert_eq!(key(&listed), key(&wanted_denies));
    let t = PersistentTrustees {
        group_sid: Some(GROUP),
        real_user_sid: "S-1-5-21-1-2-3-1000",
        previous_group_sids: &[],
        sandbox_user_sid: None,
        recorded_denies: Some(&listed),
    };
    for d in &wanted_denies {
        assert!(acl::is_persistent_ace(d, &t), "{}", d.sddl());
    }
    let fence = AceView {
        deny: true,
        inherited: false,
        flags: 0,
        mask: 0x000d_0040,
        sid: GROUP.to_owned(),
    };
    assert!(!acl::is_persistent_ace(&fence, &t));
    assert_eq!(acl::foreign_group_denies(std::slice::from_ref(&fence), &t).len(), 1);
    Ok(())
}

#[test]
fn an_unlocatable_root_is_skipped_only_with_force() -> Result<(), Box<dyn std::error::Error>> {
    let e = unlocatable(r"\\?\X:\gone", false).err().ok_or("expected an error")?;
    assert!(format!("{e:#}").contains("could not be located"));
    assert!(unlocatable(r"\\?\X:\gone", true).is_ok());
    assert_eq!(Forgot::Dropped.as_str(), "dropped");
    Ok(())
}

#[test]
fn a_kept_deny_on_a_hardlinked_file_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    let deny = SbAce::Deny(acl::DenyMask::WriteDeny);
    let e = refuse_hardlinked_deny(r"\\?\C:\r\f", deny, 2, false)
        .err()
        .ok_or("expected an error")?;
    assert!(format!("{e:#}").contains("2 hardlink(s)"));
    assert!(refuse_hardlinked_deny(r"\\?\C:\r\f", deny, 1, false).is_ok());
    // A directory's link count is not a hard-link count.
    assert!(refuse_hardlinked_deny(r"\\?\C:\r", deny, 3, true).is_ok());
    assert!(refuse_hardlinked_deny(r"\\?\C:\r", SbAce::DenyFdc, 3, false).is_ok());
    Ok(())
}

#[test]
fn capability_sid_format() -> Result<(), Box<dyn std::error::Error>> {
    let sid = capability_sid_from([
        1, 0, 0, 0, // 1
        0, 1, 0, 0, // 256
        0xff, 0xff, 0xff, 0xff, // u32::MAX
        0, 0, 0, 0, // 0
    ]);
    assert_eq!(sid, "S-1-0-1953719907-1-256-4294967295-0");
    assert!(sid.starts_with(CAPABILITY_SID_PREFIX));
    assert_eq!(CAPABILITY_TAG.to_be_bytes(), *b"tsfc");
    assert!(is_capability_sid(&sid));
    let p = crate::sid::LocalPsid::from_string(&sid)?;
    let b = p.as_bytes();
    assert_eq!((b[0], b[1]), (1, 5));
    assert_eq!(b[2..8], [0u8; 6]);
    assert_eq!(crate::sid::psid_to_string(p.as_psid())?, sid);
    assert_eq!(acl::sid_string_from_bytes(b).as_deref(), Some(sid.as_str()));
    Ok(())
}

#[test]
fn generated_capability_sids_are_valid_and_distinct() -> Result<(), Box<dyn std::error::Error>> {
    let a = new_capability_sid()?;
    let b = new_capability_sid()?;
    assert!(is_capability_sid(&a), "{a}");
    assert!(is_capability_sid(&b), "{b}");
    assert_ne!(a, b);
    Ok(())
}

#[test]
fn only_the_exact_layout_is_a_capability_sid() {
    for not in [
        "S-1-0-0",                           // Nobody
        "S-1-1-0",                           // Everyone
        "S-1-5-12",                          // RESTRICTED
        "S-1-3-4",                           // OWNER RIGHTS
        "S-1-5-32-545",                      // BUILTIN\Users
        "S-1-5-21-1-2-3-1001",               // an account
        "S-1-15-3-1024-1-2-3-4-5-6-7-8",     // a package capability
        "S-1-0-1953719907-1-2-3",            // three random parts
        "S-1-0-1953719907-1-2-3-4-5",        // five
        "S-1-0-1953719907-1-2-3-4294967296", // not a u32
        "S-1-0-1953719907-01-2-3-4",         // not canonical
        "S-1-0-1953719907-1-2-3-",           // empty part
        "S-1-0-1936880738-1-2-3-4",          // another tag
        "S-1-5-1953719907-1-2-3-4",          // another authority
        "s-1-0-1953719907-1-2-3-4",          // not canonical
        "",
    ] {
        assert!(!is_capability_sid(not), "{not}");
    }
    assert!(is_capability_sid("S-1-0-1953719907-1-2-3-4"));
}

#[test]
fn uninstall_forgets_grants_only_after_the_elevated_removal_succeeded() {
    assert_eq!(
        after_elevation(0, false),
        AfterElevation::ForgetGrants { step_failed: None }
    );
    for code in [1, 2, 23, -1, 259] {
        assert_eq!(
            after_elevation(code, false),
            AfterElevation::ForgetGrants {
                step_failed: Some(code)
            }
        );
    }
    for code in [10, 1, 2, -1, 259] {
        assert_eq!(after_elevation(code, true), AfterElevation::LeaveEverything(code));
    }
    assert_eq!(after_elevation(0, true), AfterElevation::LeaveEverything(1));
}

#[test]
fn a_pending_strip_round_trips_and_blocks_only_while_its_volume_is_mounted() -> Result<(), Box<dyn std::error::Error>> {
    let id = |serial: u64| -> Result<_, Box<dyn std::error::Error>> {
        let mut b = [0u8; 24];
        b[..8].copy_from_slice(&serial.to_le_bytes());
        b[8] = 7;
        Ok(FileId::from_bytes(&b)?)
    };
    let p = PendingStrip {
        file_id: id(0xAB)?,
        group_sid: "S-1-5-21-1-2-3-1001".to_owned(),
        path: r"\\?\E:\work|odd".to_owned(),
    };
    assert_eq!(PendingStrip::decode(&p.encode())?, p);
    assert!(PendingStrip::decode("nonsense").is_err());
    assert!(PendingStrip::decode("zz|S-1-1-0|x").is_err());

    let marker = || {
        Ok(Marker {
            pending: vec![p.clone()],
            ..Default::default()
        })
    };
    let no_db = || -> Option<Vec<String>> { Some(vec!["the database was consulted".to_owned()]) };
    assert_eq!(block_reason_from(marker(), no_db, |_| false), None);
    let serial = p.file_id.volume_serial;
    assert_eq!(block_reason_from(marker(), no_db, |s| s == serial + 1), None);
    let why = block_reason_from(marker(), no_db, |s| s == serial).ok_or("missing value")?;
    assert!(why.contains(r"E:\work"), "{why}");
    assert!(!marker()?.nothing_recorded());
    assert!(Marker::default().nothing_recorded());
    Ok(())
}

/// The re-check of pending strips: what stays listed.
#[test]
fn pending_strips_are_retried_until_the_strip_succeeds() -> Result<(), Box<dyn std::error::Error>> {
    let entry = |n: u8| -> Result<PendingStrip, Box<dyn std::error::Error>> {
        Ok(PendingStrip {
            file_id: FileId::from_bytes(&[n; 24])?,
            group_sid: "S-1-5-21-1-2-3-1001".to_owned(),
            path: format!(r"\\?\E:\root{n}"),
        })
    };
    let all = || -> Result<Vec<PendingStrip>, Box<dyn std::error::Error>> {
        Ok(vec![entry(1)?, entry(2)?, entry(3)?, entry(4)?])
    };
    let stripped = RefCell::new(Vec::<String>::new());
    let locate = |p: &PendingStrip| match p.file_id.as_bytes()[0] {
        1 => Located::VolumeAbsent,
        2 => Located::At(r"\\?\F:\moved".to_owned()),
        3 => Located::Gone,
        _ => Located::Unknown,
    };
    let (left, changed) = retry_pending_with(all()?, locate, |p, at| {
        stripped.borrow_mut().push(format!("{}@{at}", p.path));
        Ok(())
    })?;
    assert!(changed);
    assert_eq!(left, vec![entry(1)?, entry(4)?]);
    assert_eq!(*stripped.borrow(), [r"\\?\E:\root2@\\?\F:\moved"]);

    let (left, changed) = retry_pending_with(all()?, locate, |_, _| {
        Err(FenceHelperError::msg("access denied".to_string()))
    })?;
    assert_eq!(left, vec![entry(1)?, entry(2)?, entry(4)?]);
    assert!(changed, "the gone one was dropped");

    let stripped_again = std::cell::Cell::new(false);
    let (left, changed) = retry_pending_with(vec![entry(1)?], locate, |_, _| {
        stripped_again.set(true);
        Ok(())
    })?;
    assert_eq!((left, changed), (vec![entry(1)?], false));
    assert!(!stripped_again.get());

    let at = || Located::At("p".into());
    let cases: [(AtPath, Located, bool, Disposition); 16] = [
        (Recorded, Located::Unknown, false, Disposition::StripHere),
        (Recorded, Located::VolumeAbsent, true, Disposition::StripHere),
        (Nothing, at(), false, Disposition::StripAt("p".into())),
        (Another, at(), false, Disposition::StripAt("p".into())),
        (Nothing, Located::Gone, false, Disposition::Drop),
        (Another, Located::Gone, true, Disposition::Drop),
        (Nothing, Located::Unknown, false, Disposition::Fail),
        (Another, Located::Unknown, false, Disposition::Fail),
        (Nothing, Located::VolumeAbsent, false, Disposition::Pending),
        (Another, Located::VolumeAbsent, false, Disposition::Pending),
        (Unavailable, Located::VolumeAbsent, false, Disposition::Pending),
        (Unavailable, Located::Unknown, false, Disposition::Pending),
        (Unavailable, Located::VolumeAbsent, true, Disposition::Pending),
        (Unavailable, at(), false, Disposition::StripAt("p".into())),
        (Nothing, Located::VolumeAbsent, true, Disposition::Fail),
        (Another, Located::VolumeAbsent, true, Disposition::Fail),
    ];
    for (at_path, located, network, want) in cases {
        let asked = std::cell::Cell::new(false);
        let got = disposition(
            at_path,
            || {
                asked.set(true);
                located.clone()
            },
            network,
        );
        assert_eq!(got, want, "{at_path:?} / {located:?} / network={network}");
        assert_eq!(asked.get(), at_path != Recorded, "{at_path:?}: looked up by id");
    }
    Ok(())
}

#[test]
fn marker_entry_round_trips() -> Result<(), Box<dyn std::error::Error>> {
    let r = MarkerRoot {
        file_id: FileId {
            volume_serial: 0x0123_4567_89ab_cdef,
            id128: *b"0123456789abcdef",
        },
        path: r"\\?\C:\work\a|b".to_owned(),
    };
    let s = r.encode();
    assert_eq!(s.find('|'), Some(48));
    assert_eq!(MarkerRoot::decode(&s)?, r);
    for bad in ["", "|x", "zz|x", &s[1..], &s[..49]] {
        assert!(MarkerRoot::decode(bad).is_err(), "{bad}");
    }
    Ok(())
}

#[test]
fn covering_root_is_the_longest_ancestor() {
    let roots = vec![r"\\?\C:\a".to_owned(), r"\\?\C:\a\b".to_owned(), r"\\?\D:\".to_owned()];
    let c = |p: &str| covering_root(&roots, p);
    assert_eq!(c(r"\\?\C:\a"), Some(r"\\?\C:\a"));
    assert_eq!(c(r"\\?\C:\a\x"), Some(r"\\?\C:\a"));
    assert_eq!(c(r"\\?\C:\a\b\x\y"), Some(r"\\?\C:\a\b"));
    assert_eq!(c(r"\\?\D:\x"), Some(r"\\?\D:\"));
    // A sibling whose name merely starts with a root's name.
    assert_eq!(c(r"\\?\C:\ab"), None);
    assert_eq!(c(r"\\?\C:\"), None);
}

fn marker_with(ever_enabled: bool, roots: &[&str]) -> Result<Marker, FenceHelperError> {
    Ok(Marker {
        ever_enabled,
        roots: roots
            .iter()
            .map(|p| {
                Ok(MarkerRoot {
                    file_id: FileId::from_bytes(&[0u8; 24])?,
                    path: (*p).to_owned(),
                })
            })
            .collect::<Result<Vec<_>, FenceHelperError>>()?,
        ..Default::default()
    })
}

/// Every store that could not be read refuses the ordinary token.
#[test]
fn ordinary_token_is_refused_on_any_read_failure() -> Result<(), Box<dyn std::error::Error>> {
    let no_db = || -> Option<Vec<String>> { Some(vec!["database consulted".to_owned()]) };
    assert_eq!(block_reason_from(Ok(Marker::default()), no_db, |_| false), None);
    let r = block_reason_from(Err(FenceHelperError::msg("access denied".to_string())), no_db, |_| {
        false
    })
    .ok_or("missing value")?;
    assert!(r.contains("cannot be read"), "{r}");
    let r = block_reason_from(Ok(marker_with(true, &[r"\\?\C:\a"])?), no_db, |_| false).ok_or("missing value")?;
    assert!(r.contains("1 persistent write grant"), "{r}");
    let ever = || marker_with(true, &[]);
    assert_eq!(block_reason_from(ever(), || Some(Vec::new()), |_| false), None);
    let r = block_reason_from(ever(), || Some(vec![r"\\?\C:\b".to_owned()]), |_| false).ok_or("missing value")?;
    assert!(r.contains("state database records 1"), "{r}");
    let r = block_reason_from(ever(), || None, |_| false).ok_or("missing value")?;
    assert!(r.contains("missing or unreadable"), "{r}");
    Ok(())
}

#[test]
fn unreadable_dacl_is_not_an_absent_grant() -> Result<(), Box<dyn std::error::Error>> {
    const GROUP: &str = "S-1-5-21-1-2-3-1001";
    let cap = capability_sid_from([7; 16]);
    let ace = |sid: &str, deny: bool, inherited: bool| AceView {
        deny,
        inherited,
        flags: 0x3,
        mask: 0x0013_01bf,
        sid: sid.to_owned(),
    };
    let on = |aces: Vec<AceView>| unrecorded_grant_on(Ok(aces), Some(GROUP));
    assert!(unrecorded_grant_on(Err(FenceHelperError::msg("access denied".to_string())), Some(GROUP)).is_err());
    assert_eq!(on(vec![])?, UnrecordedGrant::None);
    assert_eq!(
        on(vec![ace("S-1-5-32-545", false, true), ace(GROUP, true, false)])?,
        UnrecordedGrant::None
    );
    assert_eq!(on(vec![ace(GROUP, false, false)])?, UnrecordedGrant::Own);
    assert_eq!(on(vec![ace(&cap, false, false)])?, UnrecordedGrant::Own);
    assert_eq!(
        on(vec![ace(GROUP, false, false), ace(&cap, false, true)])?,
        UnrecordedGrant::Inherited
    );
    // Without an install record only a capability SID can be told apart.
    assert_eq!(
        unrecorded_grant_on(Ok(vec![ace(GROUP, false, false)]), None)?,
        UnrecordedGrant::None
    );
    assert_eq!(
        unrecorded_grant_on(Ok(vec![ace(&cap, false, true)]), None)?,
        UnrecordedGrant::Inherited
    );
    Ok(())
}

fn setup(group: &str) -> crate::install::SetupInfo {
    crate::install::SetupInfo {
        sandbox_user: "u".to_owned(),
        sandbox_user_sid: "S-1-5-21-1".to_owned(),
        sandbox_group_sid: group.to_owned(),
        marker_version: crate::install::SETUP_VERSION,
        created_at_unix: 0,
    }
}

/// An unreadable marker must not read as an empty one: the groups it
/// names would be dropped and their entries left on disk.
#[test]
fn an_unreadable_marker_fails_the_identities() {
    let unreadable = Err(FenceHelperError::msg("registry unreadable"));
    assert!(Ids::from_records(Some(setup("S-1-5-21-2")), unreadable, "S-1-5-21-3".to_owned()).is_err());
    let marker = Marker {
        group_sid: Some("S-1-5-21-9".to_owned()),
        ..Marker::default()
    };
    let ids = Ids::from_records(Some(setup("S-1-5-21-2")), Ok(marker), "S-1-5-21-3".to_owned());
    assert_eq!(
        ids.map(|i| i.previous_group_sids).ok(),
        Some(vec!["S-1-5-21-9".to_owned()])
    );
}

/// A recorded deny whose file id cannot be read ends the forget with the
/// rows still there, instead of dropping the path from the strip.
#[test]
fn a_deny_with_a_corrupt_file_id_fails_the_forget() -> Result<(), Box<dyn std::error::Error>> {
    let conn = crate::state_db::open_memory_db_for_tests()?;
    let (root, target) = (r"\\?\C:\w", r"\\?\C:\w\deny.txt");
    conn.execute(
        "INSERT INTO persistent_denies (canonical_path, root, kind, mask, file_id) \
         VALUES (?1, ?2, 'deny', 'denyWrite', ?3)",
        params![target, root, [1u8; 5].as_slice()],
    )?;
    let ids = Ids {
        group_sid: None,
        sandbox_user_sid: None,
        real_user_sid: crate::sid::current_user_sid()?,
        previous_group_sids: Vec::new(),
    };
    let forgot = forget_one_with(&conn, &ids, root, false, |_| Located::Gone);
    assert!(forgot.is_err());
    let left: i64 = conn.query_row("SELECT COUNT(*) FROM persistent_denies", [], |r| r.get(0))?;
    assert_eq!(left, 1);
    Ok(())
}
