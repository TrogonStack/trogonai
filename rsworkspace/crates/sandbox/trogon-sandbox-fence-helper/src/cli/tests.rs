use super::*;
use crate::acl::{DenyMask, GrantMask, SbAce};
use crate::error::FenceHelperError;
use crate::logon::SeclogonDisabled;

/// `run_from_args` strips the `argv[1]` sentinel before clap so
/// the same argv shape works whether the embedder's dispatcher
/// or the standalone `trogon-sandbox-fence-helper.exe` is the entry. The CPWLW /
/// runas re-spawn hops are covered by smoke-exec.ps1; this
/// proves both shapes parse to the same `Cmd`. Uses
/// `try_parse_from` (not `run_from_args`) so the test doesn't
/// touch the host or `process::exit`.
#[test]
fn dispatch_sentinel_is_transparent_to_clap() -> Result<(), Box<dyn std::error::Error>> {
    let strip = |args: Vec<&str>| {
        let mut v: Vec<OsString> = args.into_iter().map(OsString::from).collect();
        if v.get(1).map(OsString::as_os_str) == Some(DISPATCH_ARG1.as_ref()) {
            v.remove(1);
        }
        v
    };
    // Multicall shape: argv[0] = host exe, argv[1] = sentinel.
    let with = Cli::try_parse_from(strip(vec!["host.exe", DISPATCH_ARG1, "user", "status"]))?;
    assert!(matches!(with.cmd, Cmd::User { sub: UserCmd::Status }));
    // Standalone shape: no sentinel.
    let without = Cli::try_parse_from(strip(vec!["trogon-sandbox-fence-helper.exe", "user", "status"]))?;
    assert!(matches!(without.cmd, Cmd::User { sub: UserCmd::Status }));
    // The sentinel is NOT a clap flag - without the strip, it
    // must be rejected (otherwise a typo'd dispatcher silently
    // works on `try_parse_from` while the real binary errors).
    assert!(Cli::try_parse_from(["trogon-sandbox-fence-helper.exe", DISPATCH_ARG1, "user", "status",]).is_err());
    Ok(())
}

/// The typed JSON survives `.context(…)`; other errors yield `None`.
#[test]
fn seclogon_error_json_shape() -> Result<(), Box<dyn std::error::Error>> {
    let bare = FenceHelperError::from(SeclogonDisabled { missing: false });
    assert_eq!(
        seclogon_error_json(&bare).ok_or("missing value")?,
        serde_json::json!({
            "code": "seclogon_disabled",
            "state": "disabled",
            "message": SeclogonDisabled { missing: false }.to_string(),
        }),
    );
    let wrapped = FenceHelperError::from(SeclogonDisabled { missing: true }).context("spawn runner for CA install");
    assert_eq!(
        seclogon_error_json(&wrapped).ok_or("missing value")?["state"],
        "missing",
    );
    assert!(seclogon_error_json(&FenceHelperError::msg("CreateProcessWithLogonW: other".to_string())).is_none());
    assert_eq!(EXIT_SECLOGON_DISABLED, 18);
    Ok(())
}

/// Top-level `status` parses with and without `--sublayer-guid`.
#[test]
fn status_subcommand_parses() -> Result<(), Box<dyn std::error::Error>> {
    let bare = Cli::try_parse_from(["fence-helper", "status"])?;
    assert!(matches!(bare.cmd, Cmd::Status { sublayer_guid: None }));
    let with_sl = Cli::try_parse_from([
        "fence-helper",
        "status",
        "--sublayer-guid",
        "12345678-1234-1234-1234-1234567890ab",
    ])?;
    assert!(matches!(with_sl.cmd, Cmd::Status { sublayer_guid: Some(_) }));
    Ok(())
}

/// `--quiet` on `exec` parses and defaults false. Placement
/// before `--` (where the TS wrapper puts it) is accepted.
#[test]
fn exec_quiet_flag_parses() -> Result<(), Box<dyn std::error::Error>> {
    let with = Cli::try_parse_from(["fence-helper", "exec", "--quiet", "--", "cmd.exe"])?;
    assert!(matches!(with.cmd, Cmd::Exec { quiet: true, .. }));
    let without = Cli::try_parse_from(["fence-helper", "exec", "--", "cmd.exe"])?;
    assert!(matches!(without.cmd, Cmd::Exec { quiet: false, .. }));
    Ok(())
}

/// On `exec` the flag goes before `--`, where the TS wrapper puts it.
#[test]
fn allow_loopback_flag_parses() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::try_parse_from(["fence-helper", "exec", "--allow-loopback", "--quiet", "--", "cmd.exe"])?;
    assert!(matches!(
        cli.cmd,
        Cmd::Exec {
            allow_loopback: true,
            quiet: true,
            ..
        }
    ));
    let cli = Cli::try_parse_from(["fence-helper", "exec", "--", "cmd.exe"])?;
    assert!(matches!(
        cli.cmd,
        Cmd::Exec {
            allow_loopback: false,
            ..
        }
    ));
    // After `--` it belongs to the target, not to fence-helper.
    let cli = Cli::try_parse_from(["fence-helper", "exec", "--", "cmd.exe", "--allow-loopback"])?;
    assert!(matches!(
        cli.cmd,
        Cmd::Exec { allow_loopback: false, ref target, .. } if target.len() == 2
    ));
    let cli = Cli::try_parse_from(["fence-helper", "install", "--allow-loopback"])?;
    assert!(matches!(
        cli.cmd,
        Cmd::Install {
            allow_loopback: true,
            ..
        }
    ));
    let cli = Cli::try_parse_from(["fence-helper", "install"])?;
    assert!(matches!(
        cli.cmd,
        Cmd::Install {
            allow_loopback: false,
            ..
        }
    ));
    for (argv, want) in [
        (
            &[
                "fence-helper",
                "wfp",
                "verify",
                "--target",
                "127.0.0.1:9229",
                "--allow-loopback",
            ][..],
            true,
        ),
        (
            &["fence-helper", "wfp", "verify", "--target", "127.0.0.1:49999"][..],
            false,
        ),
    ] {
        let cli = Cli::try_parse_from(argv)?;
        assert!(matches!(
            cli.cmd,
            Cmd::Wfp {
                sub: WfpCmd::Verify { allow_loopback, .. }
            } if allow_loopback == want
        ));
    }
    // There is no range form of the option.
    assert!(Cli::try_parse_from(["fence-helper", "install", "--loopback-port-range", "3000-3099"]).is_err());
    Ok(())
}

/// The flag is off unless given, and the hidden `probe` subcommands parse.
#[test]
fn write_restricted_token_flag_parses() -> Result<(), Box<dyn std::error::Error>> {
    let on = Cli::try_parse_from([
        "fence-helper",
        "exec",
        "--quiet",
        "--write-restricted-token",
        "--",
        "cmd.exe",
    ])?;
    assert!(matches!(
        on.cmd,
        Cmd::Exec {
            write_restricted_token: true,
            ..
        }
    ));
    let off = Cli::try_parse_from(["fence-helper", "exec", "--", "cmd.exe"])?;
    assert!(matches!(
        off.cmd,
        Cmd::Exec {
            write_restricted_token: false,
            ..
        }
    ));
    // After `--` the flag belongs to the target, not to fence-helper.
    let target_arg = Cli::try_parse_from(["fence-helper", "exec", "--", "cmd.exe", "--write-restricted-token"])?;
    assert!(matches!(
        target_arg.cmd,
        Cmd::Exec {
            write_restricted_token: false,
            ..
        }
    ));
    let v_on = Cli::try_parse_from([
        "fence-helper",
        "wfp",
        "verify",
        "--target",
        "127.0.0.1:1",
        "--write-restricted-token",
    ])?;
    assert!(matches!(
        v_on.cmd,
        Cmd::Wfp {
            sub: WfpCmd::Verify {
                write_restricted_token: true,
                ..
            }
        }
    ));
    let v_off = Cli::try_parse_from(["fence-helper", "wfp", "verify", "--target", "127.0.0.1:1"])?;
    assert!(matches!(
        v_off.cmd,
        Cmd::Wfp {
            sub: WfpCmd::Verify {
                write_restricted_token: false,
                ..
            }
        }
    ));
    let roots = Cli::try_parse_from([
        "fence-helper",
        "exec",
        "--write-restricted-token",
        "--write-root",
        r"C:\a",
        "--write-root",
        r"C:\b",
        "--",
        "cmd.exe",
    ])?;
    match roots.cmd {
        Cmd::Exec { write_root, .. } => assert_eq!(write_root, [r"C:\a", r"C:\b"]),
        _ => return Err("wrong subcommand".into()),
    }
    assert!(Cli::try_parse_from(["fence-helper", "exec", "--write-root", r"C:\a", "--", "cmd.exe"]).is_err());
    let v_roots = Cli::try_parse_from([
        "fence-helper",
        "wfp",
        "verify",
        "--target",
        "127.0.0.1:1",
        "--write-restricted-token",
        "--write-root",
        r"C:\a",
    ])?;
    match v_roots.cmd {
        Cmd::Wfp {
            sub: WfpCmd::Verify { write_root, .. },
        } => assert_eq!(write_root, [r"C:\a"]),
        _ => return Err("wrong subcommand".into()),
    }
    assert!(
        Cli::try_parse_from([
            "fence-helper",
            "wfp",
            "verify",
            "--target",
            "127.0.0.1:1",
            "--write-root",
            r"C:\a",
        ])
        .is_err()
    );
    let probe = Cli::try_parse_from(["fence-helper", "probe", "connect", "--target", "127.0.0.1:1"])?;
    assert!(matches!(
        probe.cmd,
        Cmd::Probe {
            sub: ProbeCmd::Connect { .. }
        }
    ));
    // What `wfp verify --write-restricted-token` runs as the child.
    let egress = Cli::try_parse_from([
        "fence-helper",
        "probe",
        "egress",
        "--target",
        "127.0.0.1:49999",
        "--anonymous-target",
        "192.0.2.7:49999",
        "--allow-loopback",
    ])?;
    assert!(matches!(
        egress.cmd,
        Cmd::Probe {
            sub: ProbeCmd::Egress {
                allow_loopback: true,
                anonymous_egress: false,
                anonymous_target: Some(_),
                ..
            }
        }
    ));
    Ok(())
}

#[test]
fn persistent_grant_subcommands_parse() -> Result<(), Box<dyn std::error::Error>> {
    let grant = |extra: &[&str]| -> Result<_, Box<dyn std::error::Error>> {
        let mut argv = vec![
            "fence-helper",
            "acl",
            "grant",
            "--holder-pid",
            "7",
            "--sandbox-user-sid",
            "S-1-5-21-1",
        ];
        argv.extend(extra);
        Ok(match Cli::try_parse_from(argv)?.cmd {
            Cmd::Acl {
                sub: AclCmd::Grant { persistent, .. },
            } => persistent,
            _ => return Err("wrong subcommand".into()),
        })
    };
    assert!(!grant(&[])?);
    assert!(grant(&["--persistent"])?);

    let forget = |argv: &[&str]| Cli::try_parse_from(argv).map(|c| c.cmd);
    assert!(matches!(
        forget(&["fence-helper", "acl", "forget", "--all", "--json"]),
        Ok(Cmd::Acl {
            sub: AclCmd::Forget {
                all: true,
                json: true,
                ..
            }
        })
    ));
    match forget(&["fence-helper", "acl", "forget", r"C:\a", r"C:\b"]) {
        Ok(Cmd::Acl {
            sub: AclCmd::Forget { all, roots, .. },
        }) => {
            assert!(!all);
            assert_eq!(roots, [r"C:\a", r"C:\b"]);
        }
        _ => return Err("wrong subcommand".into()),
    }
    // Either every root or the named ones.
    assert!(forget(&["fence-helper", "acl", "forget", "--all", r"C:\a"]).is_err());
    match forget(&["fence-helper", "acl", "forget", "--drop-pending", r"E:\w"]) {
        Ok(Cmd::Acl {
            sub: AclCmd::Forget { drop_pending, all, .. },
        }) => {
            assert!(!all);
            assert_eq!(drop_pending, [r"E:\w"]);
        }
        _ => return Err("wrong subcommand".into()),
    }
    for mixed in [
        &["fence-helper", "acl", "forget", "--all", "--drop-pending", r"E:\w"][..],
        &["fence-helper", "acl", "forget", r"C:\a", "--drop-pending", r"E:\w"][..],
        &["fence-helper", "acl", "forget", "--drop-pending"][..],
    ] {
        assert!(forget(mixed).is_err(), "{mixed:?}");
    }
    // --force is for --all only.
    assert!(matches!(
        forget(&["fence-helper", "acl", "forget", "--all", "--force"]),
        Ok(Cmd::Acl {
            sub: AclCmd::Forget { force: true, .. }
        })
    ));
    assert!(forget(&["fence-helper", "acl", "forget", "--force", r"C:\a"]).is_err());

    // The deny lists ride in the same stdin document.
    let input: AclGrantInput =
        serde_json::from_str(r#"{"read":["r"],"write":["w"],"denyRead":["dr"],"denyWrite":["dw"]}"#)?;
    assert_eq!(
        (input.read, input.write, input.deny_read, input.deny_write),
        (
            vec!["r".to_owned()],
            vec!["w".to_owned()],
            vec!["dr".to_owned()],
            vec!["dw".to_owned()]
        )
    );
    let plain: AclGrantInput = serde_json::from_str(r#"{"write":["w"]}"#)?;
    assert!(plain.deny_read.is_empty() && plain.deny_write.is_empty());
    Ok(())
}

/// `probe token` / `probe parent-access` parse only in a `test-hooks` build.
#[test]
fn profile_subcommands_parse() -> Result<(), Box<dyn std::error::Error>> {
    let parse = |argv: &[&str]| Cli::try_parse_from(argv).map(|c| c.cmd);
    match parse(&[
        "fence-helper",
        "profile",
        "grant",
        "--exclude",
        r"C:\Users\me\.myapp",
        "--exclude",
        ".other",
        "--json",
    ]) {
        Ok(Cmd::Profile {
            sub: ProfileCmd::Grant {
                exclude, json, quiet, ..
            },
        }) => {
            assert_eq!(exclude, [r"C:\Users\me\.myapp", ".other"]);
            assert!(json && !quiet);
        }
        _ => return Err("wrong subcommand".into()),
    }
    assert!(matches!(
        parse(&["fence-helper", "profile", "grant", "--quiet"]),
        Ok(Cmd::Profile {
            sub: ProfileCmd::Grant {
                quiet: true,
                replace_excludes: false,
                ..
            }
        })
    ));
    assert!(matches!(
        parse(&["fence-helper", "profile", "grant", "--replace-excludes"]),
        Ok(Cmd::Profile {
            sub: ProfileCmd::Grant {
                replace_excludes: true,
                ..
            }
        })
    ));
    match parse(&[
        "fence-helper",
        "acl",
        "grant",
        "--holder-pid",
        "7",
        "--sandbox-user-sid",
        "S-1-5-21-1",
        "--persistent",
        "--allow-profile-exclusion",
        r"C:\Users\me\.ssh",
    ]) {
        Ok(Cmd::Acl {
            sub: AclCmd::Grant {
                allow_profile_exclusion,
                ..
            },
        }) => assert_eq!(allow_profile_exclusion, [r"C:\Users\me\.ssh"]),
        _ => return Err("wrong subcommand".into()),
    }
    assert!(is_service_account("S-1-5-18"));
    assert!(!is_service_account("S-1-5-21-1-2-3-1001"));
    assert!(matches!(
        parse(&["fence-helper", "profile", "forget", "--json"]),
        Ok(Cmd::Profile {
            sub: ProfileCmd::Forget {
                json: true,
                unless_held_by_others: None,
            }
        })
    ));
    assert!(matches!(
        parse(&["fence-helper", "profile", "forget", "--unless-held-by-others", "41"]),
        Ok(Cmd::Profile {
            sub: ProfileCmd::Forget {
                unless_held_by_others: Some(41),
                ..
            }
        })
    ));
    assert!(matches!(
        parse(&[
            "fence-helper",
            "acl",
            "forget",
            "--all",
            "--unless-held-by-others",
            "41"
        ]),
        Ok(Cmd::Acl {
            sub: AclCmd::Forget {
                all: true,
                unless_held_by_others: Some(41),
                ..
            }
        })
    ));
    assert!(matches!(
        parse(&["fence-helper", "profile", "status", "--paths"]),
        Ok(Cmd::Profile {
            sub: ProfileCmd::Status { paths: true, .. }
        })
    ));
    for sub in ["grant", "status"] {
        let parsed = parse(&["fence-helper", "profile", sub, "--test-profile", r"C:\x"]);
        assert_eq!(
            parsed.is_ok(),
            cfg!(feature = "test-hooks"),
            "profile {sub} --test-profile"
        );
    }
    assert!(parse(&["fence-helper", "profile", "grant", "--include", ".ssh"]).is_err());
    Ok(())
}

#[test]
fn probe_test_hooks_follow_the_feature() {
    for sub in ["token", "parent-access"] {
        let parsed = Cli::try_parse_from(["fence-helper", "probe", sub]);
        assert_eq!(parsed.is_ok(), cfg!(feature = "test-hooks"), "probe {sub}");
    }
}

/// A path in `denyRead` and in `denyWrite` keeps the read deny,
/// whichever comes first. smoke-aces.ps1 A32 shows it on disk.
#[test]
fn a_batch_names_each_path_and_kind_once_with_the_widest_mask() {
    let read = SbAce::Deny(DenyMask::ReadDeny);
    let write = SbAce::Deny(DenyMask::WriteDeny);
    let t = |p: &str, ace: SbAce| (p.to_string(), ace);
    for (first, second) in [(read, write), (write, read)] {
        assert_eq!(
            merge_ace_targets(vec![t(r"C:\a", first), t(r"C:\a", second)]),
            vec![t(r"C:\a", read)]
        );
    }
    assert_eq!(
        merge_ace_targets(vec![
            t(r"C:\a", SbAce::Grant(GrantMask::ReadOnly)),
            t(r"C:\a", SbAce::Grant(GrantMask::Modify)),
        ]),
        vec![t(r"C:\a", SbAce::Grant(GrantMask::Modify))]
    );
    // Another kind on the same path is another row, another path
    // is another entry, and each stays where it first came up.
    assert_eq!(
        merge_ace_targets(vec![
            t(r"C:\b", write),
            t(r"C:\a", SbAce::DenyDelete),
            t(r"C:\a", write),
            t(r"C:\b", write),
            t(r"C:\a", SbAce::DenyDelete),
            t(r"C:\a", read),
        ]),
        vec![t(r"C:\b", write), t(r"C:\a", SbAce::DenyDelete), t(r"C:\a", read),]
    );
}
