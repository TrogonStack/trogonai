use super::*;
use crate::acl::{self, AceView};
use crate::error::{FenceHelperError, Result};

const TEST_KEY: &str = r"Software\trogon-sandbox\ProfileGrantTest";
/// Guests stands in for the sandbox group: resolvable, and never
/// the identity the tests run as.
const GROUP: &str = "S-1-5-32-546";
const RX: u32 = 0x0012_00a9;
const LIST: u32 = 0x0012_00a9;
const ALL: u32 = 0x001f_01ff;

fn ids() -> Result<Ids, Box<dyn std::error::Error>> {
    Ok(Ids {
        group_sid: Some(GROUP.to_owned()),
        sandbox_user_sid: None,
        real_user_sid: crate::sid::current_user_sid()?,
        previous_group_sids: Vec::new(),
    })
}

fn scratch(tag: &str) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    let d = std::env::temp_dir().join(format!("fencehelper-profile-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d)?;
    Ok(d)
}

fn canon(p: &std::path::Path) -> Result<String, Box<dyn std::error::Error>> {
    Ok(path_id::canonicalize_path(p.to_str().ok_or("missing value")?)?.0)
}

fn write(p: &std::path::Path, rel: &str) -> Result<(), Box<dyn std::error::Error>> {
    let f = p.join(rel);
    std::fs::create_dir_all(f.parent().ok_or("missing value")?)?;
    std::fs::write(f, b"x")?;
    Ok(())
}

fn junction(link: &std::path::Path, target: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    let st = std::process::Command::new("cmd")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()?;
    assert!(st.status.success(), "mklink /J: {st:?}");
    Ok(())
}

/// One of the group's ACEs: `(explicit?, deny?, flags, mask)`.
type GroupAce = (bool, bool, u8, u32);

/// The group's ACEs on `p`.
fn group_aces(p: &std::path::Path) -> Result<Vec<GroupAce>, Box<dyn std::error::Error>> {
    Ok(acl::read_aces(p.to_str().ok_or("missing value")?)?
        .into_iter()
        .filter(|a| a.sid == GROUP)
        .map(|a| (!a.inherited, a.deny, a.flags, a.mask))
        .collect())
}

fn explicit(p: &std::path::Path) -> Result<Vec<AceView>, Box<dyn std::error::Error>> {
    Ok(acl::read_aces(p.to_str().ok_or("missing value")?)?
        .into_iter()
        .filter(|a| !a.inherited)
        .collect())
}

#[test]
fn record_entries_round_trip() -> Result<(), Box<dyn std::error::Error>> {
    let id = FileId {
        volume_serial: 0x0123_4567_89ab_cdef,
        id128: *b"0123456789abcdef",
    };
    let h = Header {
        list_version: 7,
        complete: true,
        file_id: id,
        profile: r"\\?\C:\Users\a|b".to_owned(),
    };
    assert_eq!(Header::decode(&h.encode())?, h);
    for bad in ["", "2|7|1|x|p", "1|7|2|x|p", &h.encode()[..20], "1|x"] {
        assert!(Header::decode(bad).is_err(), "{bad}");
    }
    for (kind, created, file_id) in [
        (Kind::Container, false, Some(id)),
        (Kind::Granted, false, Some(id)),
        (Kind::File, false, None),
        (Kind::Denied, true, None),
    ] {
        let r = Recorded {
            path: r"\\?\C:\Users\me\a|b".to_owned(),
            kind,
            created,
            file_id,
        };
        assert_eq!(Recorded::decode(&r.encode())?, r);
    }
    for bad in ["", "-|granted|0|", "-|nope|0|p", "zz|granted|0|p", "-|granted|2|p"] {
        assert!(Recorded::decode(bad).is_err(), "{bad}");
    }
    Ok(())
}

/// The ordinary token is allowed only on a positive "no grant".
#[test]
fn ordinary_token_is_refused_while_a_grant_is_recorded_or_unreadable() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(block_reason_from(Ok(Marker::default())), None);
    let r = block_reason_from(Err(FenceHelperError::msg("access denied".to_string()))).ok_or("missing value")?;
    assert!(r.contains("cannot be read"), "{r}");
    let entry = Recorded {
        path: r"\\?\C:\Users\me".to_owned(),
        kind: Kind::Container,
        created: false,
        file_id: None,
    };
    let r = block_reason_from(Ok(Marker {
        paths: vec![entry.clone()],
        ..Default::default()
    }))
    .ok_or("missing value")?;
    assert!(r.contains("1 path(s)"), "{r}");
    let r = block_reason_from(Ok(Marker {
        header: Some(Header {
            list_version: plan::LIST_VERSION,
            complete: false,
            file_id: FileId::from_bytes(&[0u8; 24])?,
            profile: r"\\?\C:\Users\me".to_owned(),
        }),
        ..Default::default()
    }))
    .ok_or("missing value")?;
    assert!(r.contains("profile read grant exists"), "{r}");
    Ok(())
}

/// The broker's own check of a session's grants: nothing to check
/// with no grant recorded, and a refusal - never a pass - when a
/// grant is recorded and its exclusions cannot be told.
#[test]
fn session_grants_are_refused_unless_the_exclusion_list_is_there_to_clear_them()
-> Result<(), Box<dyn std::error::Error>> {
    let p = |rel: &str| format!(r"\\?\C:\Users\me\{rel}");
    let read = |rel: &str| vec![(p(rel), false)];
    let write = |rel: &str| vec![(p(rel), true)];
    let header = Header {
        list_version: plan::LIST_VERSION,
        complete: true,
        file_id: FileId::from_bytes(&[0u8; 24])?,
        profile: p(""),
    };
    let recorded = || Marker {
        header: Some(header.clone()),
        extra: Vec::new(),
        env_extra: Vec::new(),
        exclusions: vec![p(".ssh"), p("NTUSER.DAT*")],
        ungranted_only: vec![p(r"AppData\Local\Temp")],
        paths: Vec::new(),
        notes: Vec::new(),
    };
    let check = |m: Result<Marker>, g: &[(String, bool)], admitted: &[String]| {
        check_grants_against(m, g, admitted, &|_| plan::Listing::Nothing)
    };
    check(Ok(Marker::default()), &write(".ssh"), &[])?;
    let e = check(
        Err(FenceHelperError::msg("access denied".to_string())),
        &read("Documents"),
        &[],
    )
    .err()
    .ok_or("expected an error")?;
    assert!(format!("{e:#}").contains("cannot be read"), "{e:#}");
    for broken in [
        Marker {
            exclusions: Vec::new(),
            ..recorded()
        },
        Marker {
            header: None,
            exclusions: Vec::new(),
            paths: vec![Recorded {
                path: p("Documents"),
                kind: Kind::Granted,
                created: false,
                file_id: None,
            }],
            ..recorded()
        },
        Marker {
            header: Some(header.clone()),
            ..Default::default()
        },
    ] {
        let e = check(Ok(broken), &read("Documents"), &[])
            .err()
            .ok_or("expected an error")?;
        assert!(format!("{e:#}").contains("without its exclusion list"), "{e:#}");
    }
    check(Ok(recorded()), &read("Documents"), &[])?;
    check(Ok(recorded()), &write(r"AppData\Local\Temp\session"), &[])?;
    for rel in [".ssh", r".ssh\keys", "", r"AppData\Local\Temp", "AppData"] {
        assert!(check(Ok(recorded()), &read(rel), &[]).is_err(), "read {rel}");
        assert!(check(Ok(recorded()), &write(rel), &[]).is_err(), "write {rel}");
    }
    let mut batch = read("Documents");
    batch.extend(write(".ssh"));
    assert!(check(Ok(recorded()), &batch, &[]).is_err());
    check(Ok(recorded()), &read(".ssh"), &[p(".ssh")])?;
    assert!(check(Ok(recorded()), &write(".ssh"), &[p(".ssh")]).is_err());
    Ok(())
}

#[test]
fn links_and_cloud_placeholders_are_recognised_by_attribute() {
    assert_eq!(skip_reason(ATTR_DIRECTORY, true), None);
    assert_eq!(skip_reason(0x20, false), None);
    assert!(skip_reason(ATTR_DIRECTORY | ATTR_REPARSE_POINT, true).is_some());
    assert!(skip_reason(ATTR_REPARSE_POINT, false).is_some());
    for cloud in [ATTR_OFFLINE, ATTR_RECALL_ON_OPEN, ATTR_RECALL_ON_DATA_ACCESS] {
        assert!(skip_reason(ATTR_DIRECTORY | cloud, true).is_some());
        assert_eq!(skip_reason(cloud, false), None);
    }
}

#[test]
fn a_directory_is_listed_through_its_handle_and_a_junction_is_not_entered() -> Result<(), Box<dyn std::error::Error>> {
    let base = scratch("list")?;
    let prof = base.join("p");
    let outside = base.join("outside");
    write(&prof, r"sub\f.txt")?;
    write(&prof, "top.txt")?;
    write(&outside, "secret.txt")?;
    junction(&prof.join("j"), &outside)?;
    let profile = canon(&prof)?;
    let mut tree = DiskTree { profile: &profile };
    let mut entries = tree.list(&[])?;
    entries.sort_by(|a, b| a.name.long.cmp(&b.name.long));
    let got: Vec<(&str, bool, bool)> = entries
        .iter()
        .map(|e| (e.name.long.as_str(), e.is_dir, e.skip.is_some()))
        .collect();
    assert_eq!(
        got,
        [("j", true, true), ("sub", true, false), ("top.txt", false, false)]
    );
    let j = open_for_stamp(&format!("{profile}\\j"), None)?;
    assert!(skip_reason(j.attrs, j.is_dir).is_some());
    assert_ne!(
        j.id,
        path_id::capture_file_id(&canon(&outside)?)?,
        "the open followed the junction"
    );
    assert!(tree.list(&["j".to_owned()]).is_err());
    let through = format!("{profile}\\j\\secret.txt");
    let o = open_no_follow(&through, ACCESS_STAMP)?;
    assert!(verify_location(&o, &through).is_err());
    let direct = format!("{profile}\\sub");
    let o = open_no_follow(&direct, ACCESS_STAMP)?;
    verify_location(&o, &direct)?;
    std::fs::remove_dir(prof.join("j")).ok();
    std::fs::remove_dir_all(&base).ok();
    Ok(())
}

/// The system's propagation of an inheriting ACE stops at a junction: the
/// target tree's DACLs are unchanged after the write and after its removal.
#[test]
fn inherited_entries_do_not_cross_a_junction() -> Result<(), Box<dyn std::error::Error>> {
    let base = scratch("junction")?;
    let granted = base.join("granted");
    let outside = base.join("outside");
    write(&granted, r"inner\f.txt")?;
    write(&outside, r"deep\secret.txt")?;
    junction(&granted.join("j"), &outside)?;
    let targets = [outside.clone(), outside.join("deep"), outside.join(r"deep\secret.txt")];
    let before: Vec<Vec<AceView>> = targets
        .iter()
        .map(|p| Ok(acl::read_aces(p.to_str().ok_or("non-UTF-8 path")?)?))
        .collect::<Result<_, Box<dyn std::error::Error>>>()?;
    let ids = ids()?;
    let want = acl::persistent_aces(&acl::PersistentWant {
        group_sid: GROUP,
        real_user_sid: &ids.real_user_sid,
        capability_sid: None,
        deny: None,
        deny_fdc: false,
        deny_delete: false,
        profile: Some(acl::ProfileAce::Granted),
    });
    let g = canon(&granted)?;
    let o = open_for_stamp(&g, None)?;
    let target = acl::DaclTarget::Handle(o.h.raw(), &g);
    acl::write_persistent_aces_at(target, &ids.trustees(), &want, false)?;
    assert_eq!(group_aces(&granted)?, [(true, false, 0x3, RX)]);
    assert_eq!(
        group_aces(&granted.join(r"inner\f.txt"))?,
        [(false, false, 0, RX)],
        "the ACE did not reach a file below the granted directory"
    );
    let unchanged = |when: &str| -> Result<_, Box<dyn std::error::Error>> {
        let _: () = for (p, was) in targets.iter().zip(&before) {
            assert_eq!(
                &acl::read_aces(p.to_str().ok_or("missing value")?)?,
                was,
                "{when}: the DACL of {} changed through the junction",
                p.display()
            );
        };
        Ok(())
    };
    unchanged("after the grant")?;
    acl::write_persistent_aces_at(target, &ids.trustees(), &[], false)?;
    assert!(group_aces(&granted.join(r"inner\f.txt"))?.is_empty());
    unchanged("after the removal")?;
    drop(o);
    std::fs::remove_dir(granted.join("j")).ok();
    std::fs::remove_dir_all(&base).ok();
    Ok(())
}

/// An excluded name that is a link into the profile: the link is
/// left alone, and its target carries what the name would have.
#[test]
fn an_excluded_name_that_is_a_link_protects_its_target() -> Result<(), Box<dyn std::error::Error>> {
    let base = scratch("links")?;
    let prof = base.join("profile");
    for f in [
        r"dotfiles\.ssh\id_ed25519",
        r"dotfiles\.config\gh\hosts.yml",
        r"dotfiles\.config\nvim\init.lua",
        r"dotfiles\README",
        r"Documents\a.txt",
    ] {
        write(&prof, f)?;
    }
    junction(&prof.join(".ssh"), &prof.join(r"dotfiles\.ssh"))?;
    junction(&prof.join(".config"), &prof.join(r"dotfiles\.config"))?;
    let conn = crate::state_db::open_memory_db_for_tests()?;
    let key = format!("{TEST_KEY}Links");
    let store = Store { conn: &conn, key: &key };
    clear_marker(&key)?;
    let ids = ids()?;
    let layout = Layout::resolve(Some(prof.to_str().ok_or("missing value")?))?;
    let report = grant(&store, &ids, &layout, &[], false, false)?;
    assert!(report.complete, "{report:?}");
    let deny_dir = vec![(true, true, 0x3u8, ALL)];
    assert_eq!(group_aces(&prof.join(r"dotfiles\.ssh"))?, deny_dir);
    assert_eq!(group_aces(&prof.join(r"dotfiles\.config\gh"))?, deny_dir);
    assert!(
        !group_aces(&prof.join(r"dotfiles\.ssh\id_ed25519"))?
            .iter()
            .any(|a| !a.1)
    );
    assert_eq!(group_aces(&prof.join("dotfiles"))?, [(true, false, 0, LIST)]);
    assert_eq!(group_aces(&prof.join(r"dotfiles\.config"))?, [(true, false, 0, LIST)]);
    assert_eq!(group_aces(&prof.join(r"dotfiles\README"))?, [(true, false, 0, RX)]);
    assert_eq!(
        group_aces(&prof.join(r"dotfiles\.config\nvim"))?,
        [(true, false, 0x3, RX)]
    );
    for link in [r"\.ssh", r"\.config"] {
        assert!(
            report.skipped.iter().any(|s| s.path.ends_with(link)),
            "{link}: {:?}",
            report.skipped
        );
    }
    let st = status_json(&key, Ok(layout.clone()), false);
    let listed: Vec<String> = st["exclusions"]
        .as_array()
        .ok_or("missing value")?
        .iter()
        .map(|v| v.as_str().map(str::to_lowercase))
        .collect::<Option<Vec<String>>>()
        .ok_or("non-string entry")?;
    for want in [r"\dotfiles\.ssh", r"\dotfiles\.config\gh"] {
        let full = format!("{}{want}", layout.profile.to_lowercase());
        assert!(listed.contains(&full), "{full} not in {listed:?}");
    }
    let through_link = path_id::canonicalize_path(prof.join(r".ssh\id_ed25519").to_str().ok_or("missing value")?)?.0;
    let refused = check_grants_against(read_marker_at(&key), &[(through_link, false)], &[], &|_| {
        plan::Listing::Nothing
    });
    assert!(refused.is_err(), "a session read grant on the keys through the link");
    let f = forget(&store, &ids)?;
    assert!(f.failed.is_empty(), "{f:?}");
    assert!(group_aces(&prof.join(r"dotfiles\.ssh"))?.is_empty());
    std::fs::remove_dir(prof.join(".ssh")).ok();
    std::fs::remove_dir(prof.join(".config")).ok();
    std::fs::remove_dir_all(&base).ok();
    Ok(())
}

/// An excluded directory whose owner has denied reading to Everyone,
/// Authenticated Users and Users does not stop the grant.
#[test]
fn an_excluded_directory_its_owner_is_denied_reading_of_does_not_stop_the_grant()
-> Result<(), Box<dyn std::error::Error>> {
    let base = scratch("locked")?;
    let prof = base.join("profile");
    write(&prof, r".ssh\id_ed25519")?;
    write(&prof, r"Documents\a.txt")?;
    write(&prof, ".npmrc")?;
    let ssh = prof.join(".ssh");
    let me = crate::sid::current_user_sid()?;
    let locked_down = format!(
        "D:P(D;OICI;0x1200a9;;;WD)(D;OICI;0x1200a9;;;AU)(D;OICI;0x1200a9;;;BU)\
         (A;OICI;FA;;;{me})(A;OICI;FA;;;SY)"
    );
    acl::set_path_dacl_from_sddl(ssh.to_str().ok_or("missing value")?, &locked_down, "test")?;
    let ssh_canon = format!(r"{}\.ssh", canon(&prof)?);
    let o = open_for_stamp(&ssh_canon, Some(true))?;
    assert!(o.is_dir && o.attrs & ATTR_REPARSE_POINT == 0);
    assert_eq!(
        o.bare,
        open_no_follow(&ssh_canon, ACCESS_STAMP).is_err(),
        "the bare open is the fallback, and only that"
    );
    let (_, listed) = facts_from_parent(&format!(r"{}\Documents", canon(&prof)?))?;
    assert_eq!(
        listed,
        path_id::capture_file_id(&format!(r"{}\Documents", canon(&prof)?))?
    );
    drop(o);

    let conn = crate::state_db::open_memory_db_for_tests()?;
    let key = format!("{TEST_KEY}Locked");
    let store = Store { conn: &conn, key: &key };
    clear_marker(&key)?;
    let ids = ids()?;
    let layout = Layout::resolve(Some(prof.to_str().ok_or("missing value")?))?;
    for run in ["first", "refresh"] {
        let report = grant(&store, &ids, &layout, &[], false, false)?;
        assert!(report.complete, "{run}: {report:?}");
        assert!(report.unprotected.is_empty(), "{run}");
        assert!(report.unverifiable.is_empty(), "{run}: {report:?}");
        let denied = acl::read_aces(&ssh_canon)?
            .iter()
            .any(|a| a.sid == GROUP && a.deny && !a.inherited);
        let self_closed = report.self_closed.iter().any(|i| i.path == ssh_canon);
        assert!(denied != self_closed, "{run}: denied={denied} {report:?}");
        let st = status_json(&key, Ok(layout.clone()), false);
        assert_eq!(st["state"], "current", "{run}: {st}");
        assert_eq!(
            st["self_closed"].as_array().ok_or("missing value")?.len(),
            usize::from(self_closed)
        );
        assert_eq!(group_aces(&prof.join("Documents"))?, [(true, false, 0x3, RX)]);
    }
    let f = forget(&store, &ids)?;
    assert!(f.failed.is_empty(), "{f:?}");
    assert_eq!(status_json(&key, Ok(layout), false)["state"], "none");
    let after: Vec<String> = acl::read_aces(&ssh_canon)?
        .iter()
        .filter(|a| !a.inherited)
        .map(AceView::sddl)
        .collect();
    assert_eq!(after.len(), 5, "{after:?}");
    assert!(!after.iter().any(|a| a.contains(GROUP)));
    acl::set_path_dacl_from_sddl(
        ssh.to_str().ok_or("missing value")?,
        &format!("D:P(A;OICI;FA;;;{me})"),
        "test",
    )?;
    std::fs::remove_dir_all(&base).ok();
    Ok(())
}

/// Objects the caller neither owns nor may change are classed and do not
/// stop the grant. Handing objects to another owner needs an elevated run;
/// without one the test stops there.
#[test]
fn a_cargo_home_moved_inside_the_profile_has_its_credentials_protected() -> Result<(), Box<dyn std::error::Error>> {
    const KEY: &str = r"Software\trogon-sandbox\ProfileGrantTestCargoHome";
    let base = scratch("cargohome")?;
    let prof = base.join("profile");
    write(&prof, r"tools\cargo\credentials.toml")?;
    write(&prof, r"tools\cargo\config.toml")?;
    write(&prof, r"tools\cargo\registry\index\x")?;
    write(&prof, r"tools\other.txt")?;
    write(&prof, r".cargo\credentials.toml")?;
    write(&prof, r".cargo\config.toml")?;
    let conn = crate::state_db::open_memory_db_for_tests()?;
    let store = Store { conn: &conn, key: KEY };
    clear_marker(KEY)?;
    let ids = ids()?;
    let unset = Layout {
        own_env: Vec::new(),
        ..Layout::resolve(Some(prof.to_str().ok_or("missing value")?))?
    };
    let moved_to = path_id::strip_extended_prefix(&canon(&prof.join(r"tools\cargo"))?).to_owned();
    let moved = Layout {
        own_env: vec![("CARGO_HOME".to_owned(), moved_to.clone())],
        ..unset.clone()
    };
    let denied = |rel: &str| -> Result<_, Box<dyn std::error::Error>> {
        let aces = group_aces(&prof.join(rel))?;
        Ok(!aces.is_empty() && aces.iter().all(|a| a.1))
    };
    let cargo_home = |l: &Layout| {
        let st = status_json(KEY, Ok(l.clone()), false);
        assert_eq!(st["state"], "current", "{st}");
        (
            st["env"]["CARGO_HOME"].as_str().map(str::to_lowercase),
            st["env_withheld"]["CARGO_HOME"].is_string(),
        )
    };

    let report = grant(&store, &ids, &unset, &[], false, false)?;
    assert!(report.complete, "{report:?}");
    assert!(denied(r".cargo\credentials.toml")?);
    assert!(!denied(r"tools\cargo\credentials.toml")?);
    let default_home = path_id::strip_extended_prefix(&canon(&prof.join(".cargo"))?).to_lowercase();
    assert_eq!(cargo_home(&unset), (Some(default_home), false));
    assert_eq!(cargo_home(&moved), (None, true));

    let report = grant(&store, &ids, &moved, &[], false, false)?;
    assert!(report.complete, "{report:?}");
    assert!(denied(r"tools\cargo\credentials.toml")?);
    assert!(!denied(r"tools\cargo\config.toml")?);
    assert!(!group_aces(&prof.join(r"tools\other.txt"))?.is_empty());
    let m = read_marker_at(KEY)?;
    assert_eq!(
        m.env_extra,
        [r"tools\cargo\credentials", r"tools\cargo\credentials.toml"]
    );
    assert!(plan::cargo_home_protected(&m.exclusions, &moved_to));
    assert_eq!(cargo_home(&moved), (Some(moved_to.to_lowercase()), false));
    assert!(denied(r".cargo\credentials.toml")?);

    let report = grant(&store, &ids, &unset, &[], true, false)?;
    assert!(report.complete, "{report:?}");
    assert!(denied(r"tools\cargo\credentials.toml")?);
    assert_eq!(read_marker_at(KEY)?.env_extra.len(), 2);

    let outside = Layout {
        own_env: vec![(
            "CARGO_HOME".to_owned(),
            base.to_str().ok_or("missing value")?.to_owned(),
        )],
        ..unset.clone()
    };
    let report = grant(&store, &ids, &outside, &[], false, false)?;
    assert!(report.complete, "{report:?}");
    assert_eq!(read_marker_at(KEY)?.env_extra.len(), 2);
    assert_eq!(cargo_home(&outside), (None, false));

    // A forget that has begun and not ended leaves the grant
    // incomplete, with everything still recorded; the next finishes.
    let before = read_marker_at(KEY)?;
    mark_forget_begun(KEY, &before)?;
    let begun = read_marker_at(KEY)?;
    assert_eq!(begun.paths, before.paths);
    assert_eq!(status_json(KEY, Ok(unset.clone()), false)["state"], "incomplete");
    let f = forget(&store, &ids)?;
    assert!(f.failed.is_empty(), "{f:?}");
    assert!(read_marker_at(KEY)?.env_extra.is_empty());
    std::fs::remove_dir_all(&base).ok();
    Ok(())
}

#[test]
fn objects_the_caller_may_not_change_are_classed_and_do_not_stop_the_grant() -> Result<(), Box<dyn std::error::Error>> {
    let icacls = |args: &[&str]| {
        std::process::Command::new("icacls")
            .args(args)
            .output()
            .is_ok_and(|o| o.status.success())
    };
    let base = scratch("unowned")?;
    let prof = base.join("profile");
    write(&prof, r"AppData\Roaming\Microsoft\Installer\a.msi")?;
    write(&prof, r"AppData\Roaming\Microsoft\Windows\b.txt")?;
    write(&prof, r"AppData\Roaming\npm\x.cmd")?;
    write(&prof, r"Stuck\inner\c.txt")?;
    write(&prof, r".docker\config.json")?;
    write(&prof, r".kube\config")?;
    write(&prof, r".azure\tokens")?;
    write(&prof, r"Documents\a.txt")?;
    let me = crate::sid::current_user_sid()?;
    let cases = [
        (
            r"AppData\Roaming\Microsoft\Installer",
            "(A;OICI;0x1200a9;;;WD)(A;OICI;FA;;;SY)".to_owned(),
        ),
        (".docker", "(A;OICI;0x1200a9;;;BU)(A;OICI;FA;;;SY)".to_owned()),
        (".kube", "(A;OICI;FA;;;SY)".to_owned()),
        (".azure", format!("(A;OICI;0x20000;;;{me})(A;OICI;FA;;;SY)")),
    ];
    let own_back = |p: &str, sddl: &str| {
        let took = std::process::Command::new("takeown")
            .args(["/F", p])
            .output()
            .is_ok_and(|o| o.status.success());
        took && acl::set_path_dacl_from_sddl(p, &format!("D:P{sddl}"), "test").is_ok()
    };
    let disown = |p: &str, sddl: &str| {
        own_back(p, &format!("(A;;FA;;;{me})"))
            && icacls(&[p, "/setowner", "*S-1-5-18"])
            && acl::set_path_dacl_from_sddl(p, &format!("D:P{sddl}"), "test").is_ok()
    };
    let mut disowned = Vec::new();
    for (rel, sddl) in &cases {
        let p = prof.join(rel);
        let p = p.to_str().ok_or("missing value")?.to_owned();
        if !disown(&p, sddl) {
            for p in disowned.iter().chain([&p]) {
                let _ = own_back(p, &format!("(A;;FA;;;{me})"));
                let _ = icacls(&[p, "/reset", "/T", "/C", "/Q"]);
            }
            std::fs::remove_dir_all(&base).ok();
            eprintln!("skipped: needs an elevated run to change an object's owner");
            return Ok(());
        }
        disowned.push(p);
    }
    let at = |rel: &str| -> Result<String, Box<dyn std::error::Error>> { Ok(format!(r"{}\{rel}", canon(&prof)?)) };
    let conn = crate::state_db::open_memory_db_for_tests()?;
    let key = format!("{TEST_KEY}Unowned");
    let store = Store { conn: &conn, key: &key };
    clear_marker(&key)?;
    let ids = ids()?;
    let layout = Layout::resolve(Some(prof.to_str().ok_or("missing value")?))?;
    for run in ["first", "refresh"] {
        let report = grant(&store, &ids, &layout, &[], false, false)?;
        let paths = |items: &[Item]| items.iter().map(|i| i.path.clone()).collect::<Vec<_>>();
        assert!(report.complete, "{run}: {report:?}");
        assert_eq!(
            paths(&report.not_granted),
            [at(r"AppData\Roaming\Microsoft\Installer")?],
            "{run}"
        );
        assert!(group_aces(&prof.join(r"AppData\Roaming\Microsoft\Installer"))?.is_empty());
        assert_eq!(paths(&report.unprotected), [at(".docker")?], "{run}");
        assert_eq!(paths(&report.unverifiable), [at(".kube")?], "{run}");
        assert_eq!(paths(&report.self_closed), [at(".azure")?], "{run}");
        assert_eq!(
            group_aces(&prof.join(r"AppData\Roaming\Microsoft\Windows"))?,
            [(true, false, 0x3, RX)]
        );
        assert_eq!(group_aces(&prof.join("Documents"))?, [(true, false, 0x3, RX)]);
        let st = status_json(&key, Ok(layout.clone()), true);
        assert_eq!(st["state"], "current", "{run}: {st}");
        for (class, rel) in [
            ("not_granted", r"AppData\Roaming\Microsoft\Installer"),
            ("unprotected", ".docker"),
            ("unverifiable", ".kube"),
            ("self_closed", ".azure"),
        ] {
            let listed = st[class].as_array().ok_or("missing value")?;
            let at_rel = at(rel)?;
            assert_eq!(listed.len(), 1, "{run}: {class}: {st}");
            assert_eq!(listed[0]["path"], at_rel.as_str(), "{run}: {class}");
            assert!(!listed[0]["reason"].as_str().ok_or("missing value")?.is_empty());
            assert!(
                !st["paths"]
                    .as_array()
                    .ok_or("missing value")?
                    .iter()
                    .any(|p| p["path"] == at_rel.as_str()),
                "{run}: {rel} is recorded"
            );
        }
        if run == "refresh" {
            assert_eq!(report.writes, 0, "a refresh writes nothing: {report:?}");
        }
    }
    let azure = prof.join(".azure");
    let azure = azure.to_str().ok_or("missing value")?;
    let reown = |sddl: &str| assert!(disown(azure, sddl), "{sddl}");
    let class_of = |rel: &str| -> Result<String, Box<dyn std::error::Error>> {
        let st = status_json(&key, Ok(layout.clone()), false);
        let path = at(rel)?;
        Ok(["not_granted", "self_closed", "unverifiable", "unprotected"]
            .into_iter()
            .find(|c| {
                st[*c]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|i| i["path"] == path.as_str()))
            })
            .unwrap_or("none")
            .to_owned())
    };
    assert_eq!(class_of(".azure")?, "self_closed");
    reown(&format!(
        "(A;OICI;0x20000;;;{me})(A;OICI;FA;;;SY)(A;OICI;0x1200a9;;;BU)"
    ));
    assert_eq!(class_of(".azure")?, "unprotected");
    reown(&format!(
        "(A;OICI;0x20000;;;{me})(A;OICI;FA;;;SY)(A;OICIIO;0x1200a9;;;BU)"
    ));
    assert_eq!(class_of(".azure")?, "unprotected");
    reown("(A;OICI;FA;;;SY)");
    assert_eq!(class_of(".azure")?, "unverifiable");
    reown(&format!("(A;OICI;0x20000;;;{me})(A;OICI;FA;;;SY)"));
    assert_eq!(class_of(".azure")?, "self_closed");
    assert_eq!(class_of(".docker")?, "unprotected");
    assert_eq!(class_of(".kube")?, "unverifiable");

    let docs = prof.join("Documents");
    let docs = docs.to_str().ok_or("missing value")?.to_owned();
    assert!(disown(
        &docs,
        &format!("(A;OICI;0x1200a9;;;{GROUP})(A;OICI;0x1200a9;;;BU)(A;OICI;FA;;;SY)"),
    ));
    disowned.push(docs.clone());
    let report = grant(&store, &ids, &layout, &[], false, false)?;
    assert!(report.complete, "{report:?}");
    assert!(report.stale_allow.is_empty(), "{report:?}");
    let documents = at("Documents")?;
    assert!(report.skipped.iter().any(|i| i.path == documents), "{report:?}");
    assert_eq!(group_aces(&prof.join("Documents"))?, [(true, false, 0x3, RX)]);
    let st = status_json(&key, Ok(layout.clone()), true);
    assert_eq!(st["state"], "current");
    assert!(
        st["paths"]
            .as_array()
            .ok_or("missing value")?
            .iter()
            .any(|p| p["path"] == documents.as_str())
    );
    let ours = |report: &Report, path: &str| {
        report
            .unprotected
            .iter()
            .find(|i| i.path == path)
            .map(|i| (i.protected_exclusion, i.entry, i.reason.clone()))
    };
    let report = grant(&store, &ids, &layout, &[r"Documents\secret".to_owned()], true, false)?;
    assert!(report.complete && report.stale_allow.is_empty(), "{report:?}");
    let (protected, entry, reason) = ours(&report, &at("Documents")?).ok_or("missing value")?;
    assert_eq!((protected, entry), (Some(true), Some("explicit")));
    assert!(reason.contains("an exclusion lies beneath it"), "{reason}");
    let report = grant(&store, &ids, &layout, &["Documents".to_owned()], true, false)?;
    assert!(report.complete && report.stale_allow.is_empty(), "{report:?}");
    let (protected, entry, reason) = ours(&report, &at("Documents")?).ok_or("missing value")?;
    assert_eq!((protected, entry), (Some(true), Some("explicit")));
    assert!(
        reason.contains(&format!("/remove:g \"*{GROUP}\"")) && reason.contains("icacls"),
        "{reason}"
    );
    assert_eq!(group_aces(&prof.join("Documents"))?, [(true, false, 0x3, RX)]);
    let recorded = read_marker_at(&key)?;
    assert!(
        recorded
            .notes
            .iter()
            .any(|n| n.class == NoteClass::OurAllow && n.path == documents)
    );
    assert_eq!(status_json(&key, Ok(layout.clone()), false)["state"], "current");
    let report = grant(&store, &ids, &layout, &[], true, false)?;
    assert!(report.complete && report.stale_allow.is_empty(), "{report:?}");
    assert!(ours(&report, &at("Documents")?).is_none(), "{report:?}");

    let shelf = prof.join("Shelf");
    std::fs::create_dir_all(shelf.join("box"))?;
    let note = shelf.join("box").join("note.txt");
    std::fs::write(&note, b"x")?;
    let note = note.to_str().ok_or("missing value")?.to_owned();
    let by_child = [r"Shelf\box\secret".to_owned()];
    let report = grant(&store, &ids, &layout, &by_child, true, false)?;
    assert!(report.complete, "{report:?}");
    assert_eq!(group_aces(std::path::Path::new(&note))?.len(), 1);
    assert!(disown(
        &note,
        &format!("(A;;0x1200a9;;;{GROUP})(A;;0x1200a9;;;BU)(A;;FA;;;SY)"),
    ));
    disowned.push(note.clone());
    let report = grant(&store, &ids, &layout, &[], true, false)?;
    assert!(report.complete, "{report:?}");
    assert!(ours(&report, &at(r"Shelf\box\note.txt")?).is_none());
    let stale: Vec<_> = report
        .stale_allow
        .iter()
        .map(|i| (i.path.clone(), i.protected_exclusion, i.entry))
        .collect();
    assert_eq!(
        stale,
        [(at(r"Shelf\box\note.txt")?, Some(false), Some("explicit"))],
        "{report:?}"
    );
    assert!(own_back(&note, &format!("(A;;0x1200a9;;;{GROUP})(A;;FA;;;{me})")));
    let report = grant(&store, &ids, &layout, &[], true, false)?;
    assert!(report.complete && report.stale_allow.is_empty(), "{report:?}");

    let f = forget(&store, &ids)?;
    assert_eq!(
        f.failed.iter().map(|i| i.path.clone()).collect::<Vec<_>>(),
        [at("Documents")?],
        "{f:?}"
    );
    assert!(own_back(
        &docs,
        &format!("(A;OICI;0x1200a9;;;{GROUP})(A;OICI;FA;;;{me})")
    ));
    let f = forget(&store, &ids)?;
    assert!(f.failed.is_empty(), "{f:?}");
    assert_eq!(status_json(&key, Ok(layout), false)["state"], "none");
    for p in &disowned {
        let _ = own_back(p, &format!("(A;;FA;;;{me})"));
        let _ = icacls(&[p, "/reset", "/T", "/C", "/Q"]);
    }
    std::fs::remove_dir_all(&base).ok();
    Ok(())
}

/// A link to a target that does not exist yet: the rule is carried
/// there all the same, with the directory created to hold the
/// deny, so keys put there later are not readable.
#[test]
fn a_link_to_a_missing_target_protects_where_it_leads() -> Result<(), Box<dyn std::error::Error>> {
    let base = scratch("dangling")?;
    let prof = base.join("profile");
    write(&prof, r"dotfiles\README")?;
    write(&prof, r"Documents\a.txt")?;
    std::fs::create_dir_all(prof.join(r"dotfiles\.ssh"))?;
    junction(&prof.join(".ssh"), &prof.join(r"dotfiles\.ssh"))?;
    std::fs::remove_dir(prof.join(r"dotfiles\.ssh"))?;
    let conn = crate::state_db::open_memory_db_for_tests()?;
    let key = format!("{TEST_KEY}Dangling");
    let store = Store { conn: &conn, key: &key };
    clear_marker(&key)?;
    let ids = ids()?;
    let layout = Layout::resolve(Some(prof.to_str().ok_or("missing value")?))?;
    let report = grant(&store, &ids, &layout, &[], false, false)?;
    assert!(report.complete, "{report:?}");
    assert_eq!(group_aces(&prof.join("dotfiles"))?, [(true, false, 0, LIST)]);
    assert_eq!(group_aces(&prof.join(r"dotfiles\.ssh"))?, [(true, true, 0x3u8, ALL)]);
    assert_eq!(group_aces(&prof.join(r"dotfiles\README"))?, [(true, false, 0, RX)]);
    let f = forget(&store, &ids)?;
    assert!(f.failed.is_empty(), "{f:?}");
    assert!(!prof.join(r"dotfiles\.ssh").exists());
    std::fs::remove_dir(prof.join(".ssh")).ok();
    std::fs::remove_dir_all(&base).ok();
    Ok(())
}

/// A whole grant, refresh and removal on a made-up profile, read
/// back as DACLs.
#[test]
fn grant_refresh_and_forget_on_a_profile_tree() -> Result<(), Box<dyn std::error::Error>> {
    let base = scratch("grant")?;
    let prof = base.join("profile");
    let outside = base.join("outside");
    for f in [
        r"Documents\a.txt",
        r".ssh\id_ed25519",
        ".npmrc",
        "stray.txt",
        ".netrc",
        r".cargo\credentials.toml",
        r".cargo\config.toml",
        r".cargo\bin\tool.exe",
        r"AppData\Roaming\npm\x.cmd",
        r"AppData\Roaming\Microsoft\Credentials\c",
        r"AppData\Local\Temp\t.tmp",
        r"AppData\Local\Temp\session\s.txt",
        r"AppData\Local\Temp\keys\k.pem",
    ] {
        write(&prof, f)?;
    }
    write(&outside, "secret.txt")?;
    junction(&prof.join("link"), &outside)?;
    junction(&prof.join(r"Documents\inner-link"), &outside)?;
    let outside_before = explicit(&outside)?;
    let outside_all = acl::read_aces(outside.to_str().ok_or("missing value")?)?;

    let touched = [
        "",
        "Documents",
        ".ssh",
        ".npmrc",
        ".netrc",
        ".cargo",
        r".cargo\credentials.toml",
        r".cargo\config.toml",
        r".cargo\bin",
        "AppData",
        r"AppData\Roaming",
        r"AppData\Roaming\npm",
        r"AppData\Roaming\Microsoft",
        r"AppData\Roaming\Microsoft\Credentials",
        r"AppData\Local",
        r"AppData\Local\Temp",
    ];
    let snapshot = || -> Result<Vec<Vec<AceView>>, Box<dyn std::error::Error>> {
        touched.iter().map(|t| explicit(&prof.join(t))).collect()
    };
    let pre = snapshot()?;

    let conn = crate::state_db::open_memory_db_for_tests()?;
    let store = Store {
        conn: &conn,
        key: TEST_KEY,
    };
    clear_marker(TEST_KEY)?;
    let ids = ids()?;
    let layout = Layout::resolve(Some(prof.to_str().ok_or("missing value")?))?;
    let report = grant(&store, &ids, &layout, &[], false, false)?;
    assert!(report.complete, "{report:?}");
    assert!(report.complete, "{report:?}");

    let allow = |inherit: u8, mask: u32| vec![(true, false, inherit, mask)];
    let deny_dir = vec![(true, true, 0x3, ALL)];
    for c in [
        "",
        ".cargo",
        "AppData",
        r"AppData\Roaming",
        r"AppData\Roaming\Microsoft",
        r"AppData\Local",
    ] {
        assert_eq!(group_aces(&prof.join(c))?, allow(0, LIST), "container '{c}'");
    }
    for g in ["Documents", r".cargo\bin", r"AppData\Roaming\npm"] {
        assert_eq!(group_aces(&prof.join(g))?, allow(0x3, RX), "granted '{g}'");
    }
    assert_eq!(group_aces(&prof.join(r"Documents\a.txt"))?, [(false, false, 0, RX)]);
    for f in [".npmrc", r".cargo\config.toml"] {
        assert_eq!(group_aces(&prof.join(f))?, allow(0, RX), "file '{f}'");
    }
    assert!(group_aces(&prof.join("stray.txt"))?.is_empty());
    for d in [".ssh", r"AppData\Roaming\Microsoft\Credentials"] {
        assert_eq!(group_aces(&prof.join(d))?, deny_dir, "excluded '{d}'");
    }
    for t in [
        r"AppData\Local\Temp",
        r"AppData\Local\Temp\t.tmp",
        r"AppData\Local\Temp\session",
        r"AppData\Local\Temp\session\s.txt",
        r"AppData\Local\Temp\keys",
    ] {
        assert!(group_aces(&prof.join(t))?.is_empty(), "'{t}'");
    }
    for f in [".netrc", r".cargo\credentials.toml"] {
        let a = group_aces(&prof.join(f))?;
        assert!(
            a.len() == 1 && a[0].0 && a[0].1 && a[0].3 == ALL,
            "excluded file '{f}': {a:?}"
        );
    }
    assert_eq!(group_aces(&prof.join(r".ssh\id_ed25519"))?, [(false, true, 0, ALL)]);
    assert!(report.placeholders > 0);
    assert_eq!(group_aces(&prof.join(".aws"))?, deny_dir);
    assert!(
        report.skipped.iter().any(|s| s.path.to_lowercase().ends_with(r"\link")),
        "{:?}",
        report.skipped
    );
    assert_eq!(explicit(&outside)?, outside_before);
    assert_eq!(acl::read_aces(outside.to_str().ok_or("missing value")?)?, outside_all);
    assert!(group_aces(&outside.join("secret.txt"))?.is_empty());

    for excluded in [
        r".ssh\id_ed25519",
        ".netrc",
        r".cargo\credentials.toml",
        r"AppData\Roaming\Microsoft\Credentials\c",
        r"AppData\Local\Temp\t.tmp",
        r".aws\credentials",
    ] {
        let mut up = prof.join(excluded);
        while up.pop() && up.starts_with(&prof) {
            let inheriting: Vec<_> = group_aces(&up)?
                .into_iter()
                .filter(|(explicit, deny, flags, _)| *explicit && !*deny && flags & 0x3 != 0)
                .collect();
            assert!(
                inheriting.is_empty(),
                "{} carries an inheriting allow above '{excluded}': {inheriting:?}",
                up.display()
            );
        }
    }
    std::fs::remove_dir_all(prof.join(".ssh"))?;
    write(&prof, r".ssh\id_ed25519")?;
    std::fs::write(prof.join(".netrc.tmp"), b"new")?;
    std::fs::rename(prof.join(".netrc.tmp"), prof.join(".netrc"))?;
    std::fs::write(prof.join(r".cargo\c.tmp"), b"new")?;
    std::fs::rename(prof.join(r".cargo\c.tmp"), prof.join(r".cargo\credentials.toml"))?;
    for p in [".ssh", r".ssh\id_ed25519", ".netrc", r".cargo\credentials.toml"] {
        assert!(
            !group_aces(&prof.join(p))?.iter().any(|a| !a.1),
            "recreated '{p}' is allowed: {:?}",
            group_aces(&prof.join(p))?
        );
    }

    write(&prof, r"NewTop\n.txt")?;
    assert!(group_aces(&prof.join("NewTop"))?.is_empty());
    let again = grant(&store, &ids, &layout, &[], false, false)?;
    assert!(again.complete, "{again:?}");
    assert_eq!(group_aces(&prof.join("NewTop"))?, allow(0x3, RX));
    assert_eq!(group_aces(&prof.join(".ssh"))?, deny_dir);
    assert!(group_aces(&prof.join(".netrc"))?.iter().all(|a| a.1));
    assert_eq!(again.writes, 4, "{again:?}");
    let third = grant(&store, &ids, &layout, &[], false, false)?;
    assert_eq!(third.writes, 0, "{third:?}");
    assert_eq!(explicit(&outside)?, outside_before);

    let st = status_json(TEST_KEY, Ok(layout.clone()), true);
    assert_eq!(st["state"], "current", "{st}");
    assert_eq!(
        st["env"]["NPM_CONFIG_USERCONFIG"].as_str().map(str::to_lowercase),
        Some(path_id::strip_extended_prefix(&format!("{}\\.npmrc", layout.profile)).to_lowercase())
    );
    assert!(st["paths"].as_array().is_some_and(|p| !p.is_empty()));
    assert_eq!(st["replaced_files"], serde_json::json!([]));

    // Saved by write-and-rename, the file is another object and has
    // no entry: its pointer is withheld and the file is named, until
    // the next run of the grant.
    let npmrc = prof.join(".npmrc");
    let staged = prof.join(".npmrc.lock");
    std::fs::write(&staged, b"y")?;
    std::fs::rename(&staged, &npmrc)?;
    assert!(group_aces(&npmrc)?.is_empty());
    let st = status_json(TEST_KEY, Ok(layout.clone()), false);
    assert_eq!(st["state"], "current", "{st}");
    assert!(st["env"]["NPM_CONFIG_USERCONFIG"].is_null(), "{st}");
    let named: Vec<String> = st["replaced_files"]
        .as_array()
        .ok_or("missing value")?
        .iter()
        .map(|p| p.as_str().map(str::to_lowercase))
        .collect::<Option<Vec<String>>>()
        .ok_or("non-string entry")?;
    assert_eq!(named, [path_id::strip_extended_prefix(&canon(&npmrc)?).to_lowercase()]);
    let again = grant(&store, &ids, &layout, &[], false, false)?;
    assert_eq!(again.writes, 1, "{again:?}");
    let st = status_json(TEST_KEY, Ok(layout.clone()), false);
    assert!(st["env"]["NPM_CONFIG_USERCONFIG"].is_string(), "{st}");
    assert_eq!(st["replaced_files"], serde_json::json!([]));

    let with_extra = grant(
        &store,
        &ids,
        &layout,
        &["NewTop".to_owned(), r"AppData\Local\Temp\keys".to_owned()],
        false,
        false,
    )?;
    assert!(with_extra.complete, "{with_extra:?}");
    assert_eq!(group_aces(&prof.join("NewTop"))?, deny_dir);
    assert_eq!(group_aces(&prof.join(r"AppData\Local\Temp\keys"))?, deny_dir);
    assert!(group_aces(&prof.join(r"AppData\Local\Temp"))?.is_empty());
    assert!(group_aces(&prof.join(r"AppData\Local\Temp\session"))?.is_empty());
    assert!(group_aces(&prof.join(r"NewTop\n.txt"))?.iter().all(|a| a.1));

    let kept = grant(&store, &ids, &layout, &[], false, false)?;
    assert!(kept.complete, "{kept:?}");
    assert_eq!(kept.writes, 0, "{kept:?}");
    assert_eq!(group_aces(&prof.join("NewTop"))?, deny_dir);
    let st = status_json(TEST_KEY, Ok(layout.clone()), false);
    assert_eq!(
        st["extra_exclusions"],
        serde_json::json!(["NewTop", r"AppData\Local\Temp\keys"])
    );
    let listed: Vec<String> = st["exclusions"]
        .as_array()
        .ok_or("missing value")?
        .iter()
        .map(|v| v.as_str().map(str::to_lowercase))
        .collect::<Option<Vec<String>>>()
        .ok_or("non-string entry")?;
    let at = |rel: &str| format!("{}{rel}", layout.profile.to_lowercase());
    for want in [r"\newtop", r"\.ssh", r"\appdata\local\temp\keys", r"\ntuser.dat*"] {
        assert!(listed.contains(&at(want)), "{want} not in {listed:?}");
    }
    assert!(!listed.contains(&at(r"\appdata\local\temp")));
    let ungranted: Vec<String> = st["ungranted_only"]
        .as_array()
        .ok_or("missing value")?
        .iter()
        .map(|v| v.as_str().map(str::to_lowercase))
        .collect::<Option<Vec<String>>>()
        .ok_or("non-string entry")?;
    assert_eq!(ungranted, [at(r"\appdata\local\temp")]);
    let same = grant(&store, &ids, &layout, &["newtop".to_owned()], false, false)?;
    assert_eq!(same.writes, 0, "{same:?}");
    let replaced = grant(&store, &ids, &layout, &[], true, false)?;
    assert!(replaced.complete, "{replaced:?}");
    assert_eq!(group_aces(&prof.join("NewTop"))?, allow(0x3, RX));
    assert!(group_aces(&prof.join(r"AppData\Local\Temp\keys"))?.is_empty());
    assert_eq!(
        status_json(TEST_KEY, Ok(layout.clone()), false)["extra_exclusions"],
        serde_json::json!([])
    );
    assert_eq!(group_aces(&prof.join(".ssh"))?, deny_dir);

    let f = forget(&store, &ids)?;
    assert!(f.failed.is_empty(), "{f:?}");
    assert!(f.placeholders_removed > 0, "{f:?}");
    assert!(!prof.join(".aws").exists());
    for ((t, was), now) in touched.iter().zip(&pre).zip(&snapshot()?) {
        if [".ssh", ".netrc", r".cargo\credentials.toml"].contains(t) {
            assert!(group_aces(&prof.join(t))?.is_empty(), "'{t}'");
        } else {
            assert_eq!(now, was, "'{t}' after forget");
        }
    }
    assert!(group_aces(&prof.join(r"Documents\a.txt"))?.is_empty());
    assert!(group_aces(&prof.join("NewTop"))?.is_empty());
    assert!(read_marker_at(TEST_KEY)?.is_empty());
    assert!(!db_has_rows(&conn)?);
    assert_eq!(status_json(TEST_KEY, Ok(layout), false)["state"], "none");

    std::fs::remove_dir(prof.join("link")).ok();
    std::fs::remove_dir(prof.join(r"Documents\inner-link")).ok();
    std::fs::remove_dir_all(&base).ok();
    Ok(())
}
