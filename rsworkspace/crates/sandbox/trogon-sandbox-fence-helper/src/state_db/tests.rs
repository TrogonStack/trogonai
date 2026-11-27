use super::*;
use crate::error::{FenceHelperError, Result};
use std::ffi::OsString;
use std::os::windows::fs::FileExt;

#[test]
fn state_dir_rejects_empty_or_relative() -> Result<(), Box<dyn std::error::Error>> {
    // Unset or empty → error (var_os returns Some("") for a
    // present-but-empty var, which the old code accepted).
    assert!(state_dir_from_base(None, "ProgramData").is_err());
    assert!(state_dir_from_base(Some(OsString::from("")), "ProgramData").is_err());
    // Relative → error (would put the broker-only-stamped DB
    // in CWD).
    assert!(state_dir_from_base(Some(OsString::from("rel")), "ProgramData").is_err());
    // Absolute → ok.
    let ok = state_dir_from_base(Some(OsString::from(r"C:\Users\u\AppData\Local")), "ProgramData");
    assert_eq!(ok?, PathBuf::from(r"C:\Users\u\AppData\Local\trogon-sandbox"));
    Ok(())
}

/// Open an in-memory DB and run `f` against a `Locked` view
/// (autocommit, like production). Skips the named mutex + dir
/// stamp (those are integration-tested via the G-rows in
/// smoke-exec.ps1).
fn with_mem_db<R>(
    f: impl FnOnce(&mut Locked) -> Result<R, Box<dyn std::error::Error>>,
) -> Result<R, Box<dyn std::error::Error>> {
    let conn = open_db_at(
        std::path::Path::new(":memory:"),
        SESSION_SCHEMA_SQL,
        SESSION_SCHEMA_VERSION,
    )?;
    let mut db = Locked {
        conn,
        holder_pid: HolderPid(std::process::id()),
    };
    f(&mut db)
}

/// Regression: `register_broker` uses ON CONFLICT DO UPDATE,
/// not INSERT OR REPLACE - the latter would CASCADE-delete
/// this holder's existing `ace_holders` rows on a second
/// stamp/grant.
#[test]
fn second_register_broker_keeps_existing_holds() -> Result<(), Box<dyn std::error::Error>> {
    with_mem_db(|db| {
        db.register_broker()?;
        // Two holds via direct INSERT (ensure_ace needs a real
        // file; the CASCADE behavior under test is pure SQL).
        for p in [r"\\?\C:\a", r"\\?\C:\b"] {
            db.conn.execute(
                "INSERT INTO ace_holders \
                     (canonical_path, kind, pid, want_mask) \
                     VALUES (?1, 'deny', ?2, 'denyRead')",
                params![p, db.holder_pid.0 as i64],
            )?;
        }
        assert_eq!(db.my_ace_holds(None)?.len(), 2);
        // Second batch by the same holder.
        db.register_broker()?;
        // Holds intact (would be 0 with INSERT OR REPLACE).
        assert_eq!(db.my_ace_holds(None)?.len(), 2);
        Ok(())
    })?;
    Ok(())
}

#[test]
fn schema_applies_in_memory() -> Result<(), Box<dyn std::error::Error>> {
    // Session schema: bookkeeping tables only - the setup
    // record lives in the MACHINE schema since the store split.
    with_mem_db(|db| {
        let n: i64 = db.conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' \
                 AND name IN ('brokers','working_aces','ace_holders', \
                              'placeholders','sandbox_user')",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(n, 4, "session schema: 4 bookkeeping tables, no sandbox_user");
        Ok(())
    })?;
    Ok(())
}

#[test]
fn aliveness_self_is_alive() -> Result<(), Box<dyn std::error::Error>> {
    let ct = process_create_time(unsafe { GetCurrentProcess() })?;
    assert!(is_process_alive(std::process::id(), ct));
    // Same PID, wrong create time would normally be "recycled →
    // dead", but we special-case ourselves.
    assert!(is_process_alive(std::process::id(), ct + 1));
    Ok(())
}

#[test]
fn aliveness_bogus_pid_is_dead() -> Result<(), Box<dyn std::error::Error>> {
    // PID 0x7FFF_FFFE is well above any plausible live PID.
    assert!(!is_process_alive(0x7FFF_FFFE, 0));
    Ok(())
}

const TREE: &str = r"\\?\C:\tree";
const SECRET: &str = r"\\?\C:\tree\secret";
const SID: &str = "S-1-5-21-1";

/// Every probe answers "all good".
const OK_DISK: Disk = Disk {
    gate: |_, _| IdGate::Match,
    locate: |_| path_id::Located::Gone,
    converge: |_, _, _| Ok(()),
    alive: |_, _| false,
};

/// One hold of `pid` on `(canon, kind)`, with its broker and working row.
fn seed(conn: &Connection, pid: i64, canon: &str, kind: &str, mask: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO brokers (pid, process_create_time, started_at) \
         VALUES (?1, 0, 0)",
        params![pid],
    )?;
    conn.execute(
        "INSERT INTO ace_holders (canonical_path, kind, pid, want_mask) \
         VALUES (?1, ?2, ?3, ?4)",
        params![canon, kind, pid, mask],
    )?;
    conn.execute(
        "INSERT OR REPLACE INTO working_aces (canonical_path, kind, file_id, mask) \
         VALUES (?1, ?2, ?3, ?4)",
        params![canon, kind, [0u8; 24].as_slice(), mask],
    )?;
    Ok(())
}

/// `(brokers, ace_holders, working_aces)` row counts.
fn counts(conn: &Connection) -> rusqlite::Result<(i64, i64, i64)> {
    let n =
        |t: &str| -> rusqlite::Result<i64> { conn.query_row(&format!("SELECT count(*) FROM {t}"), [], |r| r.get(0)) };
    Ok((n("brokers")?, n("ace_holders")?, n("working_aces")?))
}

/// A profile operation that is going ahead is named to those it keeps out;
/// before that, after it, or under another word, it is an ordinary wait.
#[test]
fn a_profile_operation_going_ahead_is_named_to_those_it_keeps_out() -> Result<(), Box<dyn std::error::Error>> {
    let busy = |r: Result<InitMutex>| r.err().is_some_and(|e| e.is_profile_busy());
    let held = InitMutex::acquire_within(100, 400)?;
    let r = InitMutex::acquire_within(100, 400);
    assert!(r.is_err() && !busy(r));
    assert!(!profile_operation_running());
    mark_profile_operation()?;
    let started = std::time::Instant::now();
    assert!(busy(InitMutex::acquire_within(200, 5_000)));
    assert!(started.elapsed().as_millis() < 2_000, "told promptly");
    assert!(profile_operation_running());
    let file = &held.file;
    for stale in [&b"profile\n"[..], b"profile 2147483646", b"session\n"] {
        let mut word = [b'\n'; LOCK_WORD_LEN];
        word[..stale.len()].copy_from_slice(stale);
        file.seek_write(&word, LOCK_WORD_AT)?;
        let r = InitMutex::acquire_within(100, 400);
        assert!(r.is_err() && !busy(r), "{stale:?}");
        assert!(!profile_operation_running());
    }
    mark_profile_operation()?;
    drop(held);
    assert!(!profile_operation_running());
    let next = InitMutex::acquire_within(100, 400)?;
    let mut word = [0u8; LOCK_WORD_LEN];
    next.file.seek_read(&mut word, LOCK_WORD_AT)?;
    assert_eq!(profile_word_pid(&word), None);
    drop(next);
    Ok(())
}

/// A holder is alive if the process list has its pid with the recorded
/// creation time; absent or another process's, it is dead.
#[test]
fn a_holder_is_judged_by_the_process_list() -> Result<(), Box<dyn std::error::Error>> {
    let all = process_create_times()?;
    let me = std::process::id();
    assert_eq!(
        all.get(&me).copied(),
        Some(process_create_time(unsafe { GetCurrentProcess() })?)
    );
    let (&other, &created) = all
        .iter()
        .find(|&(&pid, _)| pid != me && pid > 4)
        .ok_or("missing value")?;
    assert!(is_process_alive(other, created));
    assert!(!is_process_alive(other, created + 1));
    assert!(!is_process_alive(4, 1));
    assert!(!is_process_alive(0x7FFF_FFFE, 0));
    let mut two = vec![0u8; 200];
    two[0..4].copy_from_slice(&100u32.to_le_bytes());
    two[32..40].copy_from_slice(&7i64.to_le_bytes());
    two[80..88].copy_from_slice(&11u64.to_le_bytes());
    two[132..140].copy_from_slice(&9i64.to_le_bytes());
    two[180..188].copy_from_slice(&12u64.to_le_bytes());
    let parsed = parse_process_list(&two, 0, 32, 80, 88).ok_or("missing value")?;
    assert_eq!(parsed.len(), 2);
    assert_eq!((parsed[&11], parsed[&12]), (7, 9));
    assert!(parse_process_list(&two[..150], 0, 32, 80, 88).is_none());
    assert!(parse_process_list(&two[..100], 0, 32, 80, 88).is_none());
    assert!(parse_process_list(&two[..40], 0, 32, 80, 88).is_none());
    assert!(parse_process_list(&[], 0, 32, 80, 88).is_none());
    assert!(listed_alive(&parsed, 11, 7));
    assert!(!listed_alive(&parsed, 11, 8), "its pid is another's now");
    assert!(!listed_alive(&parsed, 13, 7), "its pid is nobody's");
    assert!(listed_alive(&parsed, 11, 0));
    assert!(!listed_alive(&parsed, 13, 0));
    let mut child = std::process::Command::new("cmd")
        .args(["/c", "ping -n 3 127.0.0.1 >nul"])
        .spawn()?;
    let recorded = pid_create_time(child.id())?;
    assert!(is_process_alive(child.id(), recorded));
    assert!(alive_by_one_snapshot()?(child.id(), recorded));
    let pid = child.id();
    child.kill().ok();
    child.wait()?;
    drop(child);
    let gone = (0..50).any(|_| {
        if !is_process_alive(pid, recorded) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        false
    });
    assert!(gone);
    Ok(())
}

/// A read deny the object's own list stands in for leaves one row -
/// no deny holder, nothing to take off the object - and
/// releasing it leaves no holder at all.
#[test]
fn a_satisfied_deny_is_released_without_a_write() -> Result<(), Box<dyn std::error::Error>> {
    with_mem_db(|db| {
        let canon = r"\\?\C:\Users\me\AppData\Local\Closed";
        let pid = db.holder_pid.0 as i64;
        seed(db.conn(), pid, canon, "deny", "denyRead")?;
        db.note_satisfied_deny(canon)?;
        let kinds: Vec<String> = query_vec(
            db.conn(),
            "SELECT kind FROM ace_holders WHERE canonical_path = ?1",
            params![canon],
            |r| r.get(0),
        )?;
        assert_eq!(kinds, [KIND_DENY_SATISFIED]);
        let working: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM working_aces", [], |r| r.get(0))?;
        assert_eq!(working, 0);
        assert_eq!(live_holders_with(db.conn(), None, |_, _| true)?.len(), 1);
        let (_, failed) = db.release_aces(SID, KIND_DENY)?;
        assert_eq!(failed, 0, "nothing was written, nothing fails");
        assert_eq!(counts(db.conn())?, (0, 0, 0));
        assert!(live_holders_with(db.conn(), None, |_, _| true)?.is_empty());
        Ok(())
    })?;
    Ok(())
}

/// Who runs on a kept grant: live holders other than the caller,
/// and never one whose process is gone.
#[test]
fn kept_grant_holders_are_the_live_other_sessions() -> Result<(), Box<dyn std::error::Error>> {
    with_mem_db(|db| {
        let me = db.holder_pid.0 as i64;
        let (other, dead) = (me + 1, me + 2);
        let root = r"\\?\C:\work".to_owned();
        db.hold_kept(std::slice::from_ref(&root), Some(KEPT_PROFILE_PATH))?;
        db.hold_kept(std::slice::from_ref(&root), Some(KEPT_PROFILE_PATH))?;
        assert_eq!(counts(&db.conn)?, (1, 2, 0));
        let alone = Locked {
            conn: open_memory_db_for_tests()?,
            holder_pid: HolderPid(db.holder_pid.0),
        };
        alone.hold_kept(&[], None)?;
        assert_eq!(counts(&alone.conn)?, (0, 0, 0));
        fn alive(_pid: u32, create_time: i64) -> bool {
            create_time != 2
        }
        for (pid, create_time) in [(other, 1), (dead, 2)] {
            db.conn.execute(
                "INSERT INTO brokers (pid, process_create_time, started_at) \
                     VALUES (?1, ?2, 0)",
                params![pid, create_time],
            )?;
            for (kind, path) in [("kept_root", root.as_str()), ("profile", "p")] {
                db.conn.execute(
                    "INSERT INTO ace_holders (canonical_path, kind, pid, want_mask) \
                         VALUES (?1, ?2, ?3, '')",
                    params![path, kind, pid],
                )?;
            }
        }
        seed(&db.conn, other, TREE, "grant", "modify")?;
        let held = kept_holders_with(&db.conn, me, alive)?;
        assert_eq!(
            held,
            [
                KeptHolder {
                    pid: other as u32,
                    kind: "kept_root".into(),
                    path: root.clone()
                },
                KeptHolder {
                    pid: other as u32,
                    kind: "profile".into(),
                    path: "p".into()
                },
            ]
        );
        let all = live_holders_with(&db.conn, None, alive)?;
        assert_eq!(all.len(), 5, "{all:?}");
        assert!(all.iter().all(|h| i64::from(h.pid) != dead));
        assert!(all.iter().any(|h| h.kind == "grant" && i64::from(h.pid) == other));
        let but_me = live_holders_with(&db.conn, Some(me as u32), alive)?;
        assert_eq!(but_me.len(), 3);
        let held = kept_holders_with(&db.conn, other, alive)?;
        assert!(held.iter().all(|h| i64::from(h.pid) == me) && held.len() == 2);

        let disk = Disk { alive, ..OK_DISK };
        let report = recover_with(&db.conn, Some(SID), &disk, false)?;
        assert_eq!(report.dead_brokers, 1);
        let left: i64 = db
            .conn
            .query_row("SELECT count(*) FROM ace_holders WHERE pid = ?1", params![dead], |r| {
                r.get(0)
            })?;
        assert_eq!(left, 0);
        assert_eq!(kept_holders_with(&db.conn, me, alive)?.len(), 2);

        db.release_kept()?;
        let mine: i64 = db
            .conn
            .query_row("SELECT count(*) FROM brokers WHERE pid = ?1", params![me], |r| r.get(0))?;
        assert_eq!(mine, 0);
        assert!(kept_holders_with(&db.conn, other, alive)?.is_empty());
        assert_eq!(kept_holders_with(&db.conn, me, alive)?.len(), 2);
        Ok(())
    })?;
    Ok(())
}

#[test]
fn unreadable_release_changes_nothing() -> Result<(), Box<dyn std::error::Error>> {
    with_mem_db(|db| {
        let me = db.holder_pid.0 as i64;
        seed(&db.conn, me, TREE, "grant", "modify")?;
        let disk = Disk {
            gate: |_, _| IdGate::Unreadable,
            ..OK_DISK
        };
        let r = release_hold(&db.conn, Some(me), TREE, "grant", SID, &disk, false)?;
        assert_eq!(r, AceRelease::Unreadable);
        assert_eq!(r.as_str(), "unreadable");
        assert_eq!(counts(&db.conn)?, (1, 1, 1));
        let r = release_hold(&db.conn, Some(me), TREE, "grant", SID, &OK_DISK, false)?;
        assert_eq!(r, AceRelease::Revoked);
        assert_eq!(counts(&db.conn)?, (1, 0, 0));
        Ok(())
    })?;
    Ok(())
}

#[test]
fn failed_converge_rolls_the_rows_back() -> Result<(), Box<dyn std::error::Error>> {
    with_mem_db(|db| {
        let me = db.holder_pid.0 as i64;
        seed(&db.conn, me, TREE, "grant", "modify")?;
        let disk = Disk {
            converge: |_, _, _| Err(FenceHelperError::msg("SetNamedSecurityInfoW failed".to_string())),
            ..OK_DISK
        };
        assert!(release_hold(&db.conn, Some(me), TREE, "grant", SID, &disk, false).is_err());
        assert_eq!(counts(&db.conn)?, (1, 1, 1));
        Ok(())
    })?;
    Ok(())
}

#[test]
fn moved_grant_is_missing_only_when_positively_gone() -> Result<(), Box<dyn std::error::Error>> {
    with_mem_db(|db| {
        let me = db.holder_pid.0 as i64;
        seed(&db.conn, me, TREE, "grant", "modify")?;
        let disk = Disk {
            gate: |_, _| IdGate::Mismatch,
            locate: |_| path_id::Located::Unknown,
            ..OK_DISK
        };
        let r = release_hold(&db.conn, Some(me), TREE, "grant", SID, &disk, false)?;
        assert_eq!(r, AceRelease::Unreadable);
        assert_eq!(counts(&db.conn)?, (1, 1, 1));
        let disk = Disk {
            gate: |_, _| IdGate::Mismatch,
            locate: |_| path_id::Located::At(r"\\?\C:\moved".into()),
            converge: |_, _, _| Err(FenceHelperError::msg("converge failed".to_string())),
            ..OK_DISK
        };
        assert!(release_hold(&db.conn, Some(me), TREE, "grant", SID, &disk, false).is_err());
        assert_eq!(counts(&db.conn)?, (1, 1, 1));
        let disk = Disk {
            gate: |_, _| IdGate::Mismatch,
            ..OK_DISK
        };
        let r = release_hold(&db.conn, Some(me), TREE, "grant", SID, &disk, false)?;
        assert_eq!(r, AceRelease::Gone);
        assert_eq!(r.as_str(), "gone");
        assert_eq!(counts(&db.conn)?, (1, 0, 0));
        Ok(())
    })?;
    Ok(())
}

#[test]
fn recovery_keeps_a_holders_denies_while_its_grant_is_pending() -> Result<(), Box<dyn std::error::Error>> {
    with_mem_db(|db| {
        let c = &db.conn;
        seed(c, 41, TREE, "grant", "modify")?;
        seed(c, 41, SECRET, "deny", "denyWrite")?;
        seed(c, 41, TREE, "deny_fdc", "fdc")?;
        seed(c, 42, r"\\?\C:\other", "grant", "read")?;
        seed(c, 42, r"\\?\C:\other\x", "deny", "denyRead")?;

        let disk = Disk {
            gate: |p, _| {
                if p == TREE { IdGate::Unreadable } else { IdGate::Match }
            },
            ..OK_DISK
        };
        let rep = recover_with(c, Some(SID), &disk, false)?;
        assert_eq!(rep.grants_pending, vec![TREE.to_string()]);
        // 42 released and pruned; 41 untouched and still registered.
        assert_eq!(rep.dead_brokers, 1);
        assert_eq!(rep.aces_revoked, 2);
        assert_eq!(counts(c)?, (1, 3, 3));

        let disk = Disk {
            converge: |_, p, _| {
                if p == TREE {
                    return Err(FenceHelperError::msg("converge failed".to_string()));
                }
                Ok(())
            },
            ..OK_DISK
        };
        let rep = recover_with(c, Some(SID), &disk, false)?;
        assert_eq!(rep.grants_pending, vec![TREE.to_string()]);
        assert_eq!(counts(c)?, (1, 3, 3));

        let rep = recover_with(c, Some(SID), &OK_DISK, false)?;
        assert!(rep.grants_pending.is_empty());
        assert_eq!((rep.dead_brokers, rep.aces_revoked), (1, 3));
        assert_eq!(counts(c)?, (0, 0, 0));
        Ok(())
    })?;
    Ok(())
}

#[test]
fn recovery_keeps_holderless_denies_while_any_grant_is_pending() -> Result<(), Box<dyn std::error::Error>> {
    with_mem_db(|db| {
        let c = &db.conn;
        seed(c, 41, TREE, "grant", "modify")?;
        seed(c, 41, SECRET, "deny", "denyWrite")?;
        c.execute("DELETE FROM brokers", [])?;
        assert_eq!(counts(c)?, (0, 0, 2));

        let disk = Disk {
            gate: |p, _| {
                if p == TREE { IdGate::Unreadable } else { IdGate::Match }
            },
            ..OK_DISK
        };
        let rep = recover_with(c, Some(SID), &disk, false)?;
        assert_eq!(rep.grants_pending, vec![TREE.to_string()]);
        assert_eq!(counts(c)?, (0, 0, 2));

        let rep = recover_with(c, Some(SID), &OK_DISK, false)?;
        assert!(rep.grants_pending.is_empty());
        assert_eq!(counts(c)?, (0, 0, 0));
        Ok(())
    })?;
    Ok(())
}

#[test]
fn deny_release_is_refused_while_the_holder_has_a_grant() -> Result<(), Box<dyn std::error::Error>> {
    with_mem_db(|db| {
        let me = db.holder_pid.0 as i64;
        seed(&db.conn, me, TREE, "grant", "modify")?;
        seed(&db.conn, me, SECRET, "deny", "denyWrite")?;
        let (out, failed) = db.release_aces(SID, KIND_DENY)?;
        assert_eq!(out, vec![(SECRET.to_string(), AceRelease::GrantPending)]);
        assert_eq!(failed, 0);
        assert!(out[0].1.is_pending());
        assert_eq!(out[0].1.as_str(), "grantPending");
        assert_eq!(counts(&db.conn)?, (1, 2, 2));
        Ok(())
    })?;
    Ok(())
}

#[test]
fn forced_recovery_drops_only_a_grant_that_cannot_be_located() -> Result<(), Box<dyn std::error::Error>> {
    with_mem_db(|db| {
        let c = &db.conn;
        seed(c, 41, TREE, "grant", "modify")?;
        seed(c, 41, SECRET, "deny", "denyWrite")?;
        let lost = Disk {
            gate: |p, _| {
                if p == TREE { IdGate::Mismatch } else { IdGate::Match }
            },
            locate: |_| path_id::Located::Unknown,
            ..OK_DISK
        };
        let rep = recover_with(c, Some(SID), &lost, false)?;
        assert_eq!(rep.grants_pending, vec![TREE.to_string()]);
        assert!(rep.grants_dropped.is_empty());
        assert_eq!(counts(c)?, (1, 2, 2));

        // A path that merely cannot be opened is not dropped.
        let closed = Disk {
            gate: |p, _| {
                if p == TREE { IdGate::Unreadable } else { IdGate::Match }
            },
            ..OK_DISK
        };
        let rep = recover_with(c, Some(SID), &closed, true)?;
        assert!(rep.grants_dropped.is_empty());
        assert_eq!(counts(c)?, (1, 2, 2));

        let rep = recover_with(c, Some(SID), &lost, true)?;
        assert_eq!(rep.grants_dropped, vec![TREE.to_string()]);
        assert!(rep.grants_pending.is_empty());
        assert_eq!((rep.dead_brokers, rep.aces_revoked), (1, 1));
        assert_eq!(counts(c)?, (0, 0, 0));
        Ok(())
    })?;
    Ok(())
}

#[test]
fn a_reused_pid_does_not_adopt_the_dead_brokers_holds() -> Result<(), Box<dyn std::error::Error>> {
    with_mem_db(|db| {
        let me = db.holder_pid.0 as i64;
        seed(&db.conn, me, TREE, "grant", "modify")?;
        db.register_broker_at(7)?;
        let holds_of = |pid: i64| -> rusqlite::Result<i64> {
            db.conn
                .query_row("SELECT count(*) FROM ace_holders WHERE pid = ?1", params![pid], |r| {
                    r.get(0)
                })
        };
        assert_eq!((holds_of(me)?, holds_of(-1)?), (0, 1));
        // The same process registering again moves nothing.
        db.register_broker_at(7)?;
        assert_eq!(counts(&db.conn)?, (2, 1, 1));
        // Recovery releases the set-aside holds whatever the probe says.
        let disk = Disk {
            alive: |_, _| true,
            ..OK_DISK
        };
        let rep = recover_with(&db.conn, Some(SID), &disk, false)?;
        assert_eq!((rep.dead_brokers, rep.aces_revoked), (1, 1));
        assert_eq!(counts(&db.conn)?, (1, 0, 0));
        Ok(())
    })?;
    Ok(())
}

#[test]
fn recovery_without_a_sandbox_identity_leaves_holds_in_place() -> Result<(), Box<dyn std::error::Error>> {
    with_mem_db(|db| {
        let c = &db.conn;
        seed(c, 41, TREE, "grant", "modify")?;
        c.execute(
            "INSERT INTO brokers (pid, process_create_time, started_at) \
             VALUES (43, 0, 0)",
            [],
        )?;
        let rep = recover_with(c, None, &OK_DISK, false)?;
        // Only the hold-less dead broker is pruned.
        assert_eq!(rep.dead_brokers, 1);
        assert_eq!(counts(c)?, (1, 1, 1));
        Ok(())
    })?;
    Ok(())
}
