use super::*;
use crate::error::FenceHelperError;

/// The share a path is on, for a lookup by file id there.
#[test]
fn the_root_of_a_share_is_its_server_and_share_or_its_drive() {
    assert_eq!(
        share_root(r"\\?\UNC\srv\data\a\b").as_deref(),
        Some(r"\\?\UNC\srv\data\")
    );
    assert_eq!(share_root(r"\\?\UNC\srv\data").as_deref(), Some(r"\\?\UNC\srv\data\"));
    assert_eq!(share_root(r"\\?\Z:\a\b").as_deref(), Some(r"\\?\Z:\"));
    assert_eq!(share_root(r"\\?\UNC\srv"), None);
    assert_eq!(share_root(r"\\srv\data\a"), None);
    assert_eq!(share_root(r"\\?\Volume{x}\a"), None);
}

/// The table of open errors.
#[test]
fn open_errors_are_absence_unavailability_or_failure() {
    let win32 = |code: u32| classify_open_failure(0x8007_0000 | code);
    for code in [2, 3] {
        assert_eq!(win32(code), OpenFailure::NotFound, "{code}");
    }
    for code in [
        15, 20, 21, 51, 53, 55, 59, 64, 67, 433, 1005, 1110, 1112, 1167, 1203, 1231, 1232, 1617,
    ] {
        assert_eq!(win32(code), OpenFailure::VolumeUnavailable, "{code}");
    }
    assert_eq!(classify_open_failure(0x8031_0000), OpenFailure::VolumeUnavailable);
    for code in [0, 1, 5, 32, 87, 123, 1117, 1920] {
        assert_eq!(win32(code), OpenFailure::Other, "{code}");
    }
    for hr in [0, 5, 0x8031_0001, 0x8000_4005, 0xC000_0022] {
        assert_eq!(classify_open_failure(hr), OpenFailure::Other, "{hr:#x}");
    }
    let e = FenceHelperError::from(windows::core::Error::from(windows::Win32::Foundation::ERROR_NOT_READY))
        .context("open 'E:\\work'");
    assert_eq!(open_failure(&e), OpenFailure::VolumeUnavailable);
    assert_eq!(
        open_failure(&FenceHelperError::msg("no system error".to_string())),
        OpenFailure::Other
    );
}

/// Volumes are found by enumeration, not by drive letter.
#[test]
fn the_system_volume_is_found_among_the_volumes() -> Result<(), Box<dyn std::error::Error>> {
    let roots = local_volume_roots();
    assert!(
        roots.iter().all(|r| r.starts_with(r"\\?\Volume{") && r.ends_with('\\')),
        "{roots:?}"
    );
    let here = capture_file_id(std::env::temp_dir().to_str().ok_or("missing value")?)?;
    assert!(volume_mounted(here.volume_serial));
    assert!(!volume_mounted(here.volume_serial ^ 0x5a5a_5a5a_5a5a_5a5a));
    match locate_by_file_id_local(&here) {
        Located::At(p) => assert_eq!(capture_file_id(&p)?, here),
        other => return Err(format!("{other:?}").into()),
    }
    let mut elsewhere = here;
    elsewhere.volume_serial ^= 0x5a5a_5a5a_5a5a_5a5a;
    assert_eq!(locate_by_file_id_local(&elsewhere), Located::VolumeAbsent);
    Ok(())
}

#[test]
fn canonicalize_rejects_globs() -> Result<(), Box<dyn std::error::Error>> {
    for p in ["C:\\foo\\*.txt", "C:\\foo\\bar?.txt"] {
        assert!(matches!(canonicalize_path(p), Err(CanonError::Glob)), "{p}");
    }
    assert!(matches!(
        canonicalize_path(r"C:\fence-helper-no-such-path"),
        Err(CanonError::NotFound(_))
    ));
    Ok(())
}

/// `create_placeholder_chain` records intent before creating,
/// builds the missing dirs + leaf, and returns exactly what it
/// created; a second call is idempotent (`AlreadyExists`
/// accepted, nothing new returned).
#[test]
fn placeholder_chain_round_trip() -> Result<(), Box<dyn std::error::Error>> {
    let base = std::env::temp_dir().join(format!("fence-ph-{}", std::process::id()));
    std::fs::create_dir_all(&base)?;
    let leaf = base.join("a").join("b").join("secret.txt");
    let mut recorded = Vec::new();
    let mut rec = |c: &str| {
        // Intent-first: not on disk yet when recorded.
        assert!(!std::path::Path::new(c).exists(), "{c}");
        recorded.push(c.to_owned());
        Ok(())
    };
    let (leaf_canon, chain) = create_placeholder_chain(&leaf.display().to_string(), false, &mut rec)?;
    // Two dirs + one leaf file, root-first; leaf canonical.
    assert_eq!(chain.len(), 3);
    assert!(chain[0].is_dir && chain[1].is_dir && !chain[2].is_dir);
    assert!(leaf_canon.starts_with(r"\\?\"), "{leaf_canon}");
    assert_eq!(leaf_canon, chain[2].canon);
    assert_eq!(recorded.len(), 3);
    assert!(leaf.is_file());
    // Idempotent: second call finds everything already present
    // → returns nothing new; leaf canon unchanged.
    let (again_canon, again) = create_placeholder_chain(&leaf.display().to_string(), false, |_| Ok(()))?;
    assert!(again.is_empty(), "got {again:?}");
    assert_eq!(again_canon, leaf_canon);
    // Trailing separator ⇒ directory leaf.
    let dir_leaf = format!("{}\\", base.join("d").display());
    let (dc, _) = create_placeholder_chain(&dir_leaf, true, |_| Ok(()))?;
    assert!(base.join("d").is_dir());
    assert!(dc.starts_with(r"\\?\"), "{dc}");
    std::fs::remove_dir_all(&base).ok();
    Ok(())
}

#[test]
fn canonicalize_round_trip_self() -> Result<(), Box<dyn std::error::Error>> {
    // The test binary's own path is a real file we definitely
    // can open.
    let exe = std::env::current_exe()?;
    let (canon, is_dir) = canonicalize_path(&exe.display().to_string())?;
    assert!(canon.starts_with(r"\\?\"), "got {canon}");
    assert!(!is_dir);
    // Round-trip: canonicalizing the canonical path is a no-op.
    let (again, _) = canonicalize_path(&canon)?;
    assert_eq!(canon, again);
    Ok(())
}

/// The parent of a file directly under a drive root IS the
/// volume root. Touching the volume root's DACL (even with an
/// additive `DenyFdc` ACE) is out of scope - so
/// `canonical_parent_of` must return `None` for a top-level
/// child. Rust's `Path::parent()` returns the root (not
/// `None`) in this case, so the helper has to recognize and
/// reject it.
#[test]
fn parent_at_volume_root_is_not_stampable() {
    for p in [r"\\?\C:\foo.txt", r"\\?\C:\ProgramData", r"\\?\D:\x"] {
        assert_eq!(
            canonical_parent_of(p),
            None,
            "would stamp the volume root for top-level child {p:?}: \
             got {:?}",
            canonical_parent_of(p),
        );
    }
    // Anchors (already pass today, must keep passing):
    // the root itself has no parent; a nested path's parent
    // is the immediate directory.
    assert_eq!(canonical_parent_of(r"\\?\C:\"), None);
    assert_eq!(canonical_parent_of(r"\\?\C:\a\b").as_deref(), Some(r"\\?\C:\a"),);
}

#[test]
fn lossy_canonical_path_detected() {
    // Lone high surrogate → from_utf16_lossy substitutes
    // U+FFFD, so the round-trip check must reject it.
    let bad = [0x0041, 0xD800, 0x0042];
    assert!(!utf16_roundtrips(&bad, &String::from_utf16_lossy(&bad)));
    // A valid surrogate PAIR (U+1F600 = D83D DE00) and plain
    // ASCII both round-trip.
    let pair = [0x0041, 0xD83D, 0xDE00, 0x0042];
    assert!(utf16_roundtrips(&pair, &String::from_utf16_lossy(&pair)));
    let ok = [0x0041, 0x0042];
    assert!(utf16_roundtrips(&ok, &String::from_utf16_lossy(&ok)));
}

#[test]
fn unc_and_extended_prefix() {
    for p in [
        r"\\server\share\x",
        "//server/share/x",
        r"\\?\UNC\server\share",
        r"\\?\unc\server\share",
        "//?/UNC/server/share",
    ] {
        assert!(is_unc_path(p), "should be UNC: {p}");
    }
    for p in [r"\\?\C:\x", "//?/C:/x", r"\\.\PIPE\x", r"C:\x", "/tmp/x"] {
        assert!(!is_unc_path(p), "should NOT be UNC: {p}");
    }
    assert_eq!(strip_extended_prefix(r"\\?\C:\y"), r"C:\y");
    assert_eq!(strip_extended_prefix("//?/C:/y"), "C:/y");
    assert_eq!(strip_extended_prefix(r"\\?\UNC\srv\s"), r"srv\s");
    assert_eq!(strip_extended_prefix(r"C:\y"), r"C:\y");
}
