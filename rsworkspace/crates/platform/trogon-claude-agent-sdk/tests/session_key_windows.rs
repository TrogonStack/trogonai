#![cfg(feature = "test-support")]

use serde_json::Value;
use trogon_claude_agent_sdk::{SessionKey, SessionKeyError, SessionSubpath, SummaryDialect, test_support::windows};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Goldens captured from the real TypeScript `sdk.mjs` (Node `path.win32`) and Python 0.2.181 (`ntpath` and
/// `PureWindowsPath`) transcript path functions. The notes inside each file say how.
const KEYS: &str = include_str!("fixtures/session_key/transcript_paths_windows.json");
const FOLD: &str = include_str!("fixtures/session_key/fold_names.json");
const RESUME: &str = include_str!("fixtures/session_key/resume_windows.json");

fn text<'a>(value: &'a Value, field: &str) -> Result<&'a str, String> {
    value[field].as_str().ok_or_else(|| format!("missing {field}"))
}

fn expect_key(derived: Result<SessionKey, SessionKeyError>, expected: &Value, context: &str) -> TestResult {
    if expected.is_null() {
        assert!(derived.is_err(), "{context} should have no key");
        return Ok(());
    }
    let key = derived.map_err(|error| format!("{context}: {error}"))?;
    assert_eq!(key.project_key().as_str(), text(expected, "project_key")?, "{context}");
    assert_eq!(key.session_id().as_str(), text(expected, "session_id")?, "{context}");
    assert_eq!(
        key.subpath().map(SessionSubpath::as_str),
        expected["subpath"].as_str(),
        "{context}"
    );
    Ok(())
}

#[test]
fn windows_transcript_path_keys_match_the_typescript_sdk() -> TestResult {
    let goldens: Value = serde_json::from_str(KEYS)?;
    let cwd = text(&goldens, "cwd")?;
    let cases = goldens["cases"].as_array().ok_or("missing cases")?;
    let mut mapped = 0;
    for case in cases {
        let (file, directory) = (text(case, "file_path")?, text(case, "projects_dir")?);
        let derived = windows::transcript_key(file, directory, cwd, SummaryDialect::TypeScript);
        mapped += usize::from(derived.is_ok());
        expect_key(derived, &case["typescript"], &format!("{file:?} under {directory:?}"))?;
    }
    assert!(mapped > 50, "the table should map many paths");
    assert!(cases.len() > mapped + 20, "the table should also reject paths");
    Ok(())
}

/// Python reads a part with a colon second, such as `a:x`, as a drive in `ntpath.join` and `PureWindowsPath`, so its
/// keys differ from TypeScript's exactly there. The golden is CPython 3.12.16, which the Python dialect follows;
/// `python_since_3_13` records the rows where later CPython joins the parts differently.
#[test]
fn windows_python_dialect_keys_match_the_python_sdk_and_differ_from_typescript_at_drive_like_parts() -> TestResult {
    let goldens: Value = serde_json::from_str(KEYS)?;
    let cwd = text(&goldens, "cwd")?;
    let cases = goldens["cases"].as_array().ok_or("missing cases")?;
    let mut disagreements = 0;
    let mut newer_cpython = 0;
    for case in cases {
        let (file, directory) = (text(case, "file_path")?, text(case, "projects_dir")?);
        let context = format!("{file:?} under {directory:?}");
        expect_key(
            windows::transcript_key(file, directory, cwd, SummaryDialect::Python),
            &case["python"],
            &context,
        )?;
        if case["python"] != case["typescript"] {
            disagreements += 1;
            assert!(
                file.split('\\').skip(3).any(|part| part.chars().nth(1) == Some(':')),
                "{context}"
            );
        }
        newer_cpython += usize::from(case.get("python_since_3_13").is_some());
    }
    assert!(disagreements >= 10, "the table should pin the drive-like parts");
    assert!(newer_cpython >= 5, "the table should record the later CPython result");
    Ok(())
}

#[test]
fn windows_typescript_resume_guard_matches_the_sdk() -> TestResult {
    let goldens: Value = serde_json::from_str(RESUME)?;
    let session_dir = text(&goldens, "session_dir")?;
    let cases = goldens["subpaths"].as_array().ok_or("missing subpaths")?;
    assert!(cases.len() > 50);
    let mut skipped = 0;
    for case in cases {
        let subpath = text(case, "subpath")?;
        let accepts = case["typescript_accepts"].as_bool().ok_or("missing verdict")?;
        skipped += usize::from(!accepts);
        assert_eq!(
            windows::typescript_subpath_is_unsafe(subpath, session_dir),
            !accepts,
            "{subpath:?}"
        );
    }
    assert!(skipped > 10, "the table should skip unsafe subpaths");
    Ok(())
}

#[test]
fn windows_python_resume_guard_matches_the_sdk_and_checked_only_adds_rust_rules() -> TestResult {
    let goldens: Value = serde_json::from_str(RESUME)?;
    let cases = goldens["subpaths"].as_array().ok_or("missing subpaths")?;
    for case in cases {
        let subpath = text(case, "subpath")?;
        let accepts = case["python_accepts"].as_bool().ok_or("missing verdict")?;
        assert_eq!(windows::python_subpath_is_safe(subpath), accepts, "{subpath:?}");
        if !accepts {
            assert!(!windows::checked_subpath_is_safe(subpath, true), "{subpath:?}");
        }
    }
    for rust_only in ["a//b", "a/", "CON", "a/nul.txt", "a/b.", "a|b"] {
        assert!(windows::python_subpath_is_safe(rust_only), "{rust_only:?}");
        assert!(!windows::checked_subpath_is_safe(rust_only, true), "{rust_only:?}");
    }
    assert!(windows::checked_subpath_is_safe("subagents/agent-1", true));
    Ok(())
}

/// `checked` follows the platform: Windows reads any character before a colon as a drive, POSIX only a letter.
#[test]
fn checked_subpath_reads_drives_as_the_platform_does() {
    for subpath in ["1:x", "\u{e9}:x", "_:b/c", "C::x"] {
        assert!(!windows::checked_subpath_is_safe(subpath, true), "{subpath:?}");
    }
    for subpath in ["1:x", "\u{e9}:x", "_:b/c"] {
        assert!(windows::checked_subpath_is_safe(subpath, false), "{subpath:?}");
    }
    for subpath in ["C:x", "z:/x", "C::x"] {
        assert!(!windows::checked_subpath_is_safe(subpath, false), "{subpath:?}");
    }
    assert!(windows::checked_subpath_is_safe("a//b", false));
    assert!(windows::checked_subpath_is_safe("CON", false));
}

/// Both SDKs lowercase a project directory name on Windows and compare it exactly elsewhere.
#[test]
fn directory_names_fold_as_both_sdks_do_on_windows_only() -> TestResult {
    let goldens: Value = serde_json::from_str(FOLD)?;
    let names = goldens["names"].as_array().ok_or("missing names")?;
    assert!(names.len() > 10);
    for entry in names {
        let name = text(entry, "name")?;
        assert_eq!(
            windows::fold_directory_name(name, true),
            text(entry, "windows")?,
            "{name:?}"
        );
        assert_eq!(
            windows::fold_directory_name(name, false),
            text(entry, "posix")?,
            "{name:?}"
        );
    }
    Ok(())
}

#[test]
fn windows_transcript_paths_outside_the_projects_or_misshapen_report_typed_errors() {
    let outside = windows::transcript_key(
        "D:\\x\\p\\s.jsonl",
        "C:\\projects",
        "C:\\work",
        SummaryDialect::TypeScript,
    );
    assert!(matches!(outside, Err(SessionKeyError::OutsideProjects)));
    let shape = windows::transcript_key(
        "C:\\projects\\p\\s.txt",
        "C:\\projects",
        "C:\\work",
        SummaryDialect::TypeScript,
    );
    assert!(matches!(shape, Err(SessionKeyError::UnrecognizedPath)));
}
