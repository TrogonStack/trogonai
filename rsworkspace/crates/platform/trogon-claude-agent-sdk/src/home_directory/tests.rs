use std::{collections::BTreeMap, ffi::OsString, path::PathBuf};

use serde_json::Value;

use super::*;
use crate::test_support::{FixedEnvironment, WithPasswd};

const GOLDEN: &str = include_str!("../../tests/fixtures/home_directory/golden.json");
const PLACEHOLDER: &str = "<PASSWD_HOME>";
const PASSWD_HOME: &str = "/passwd/home";

fn golden() -> Value {
    serde_json::from_str(GOLDEN).expect("golden fixture parses")
}

fn text(value: &Value) -> String {
    value.as_str().expect("string").replace(PLACEHOLDER, PASSWD_HOME)
}

fn environment(
    variables: &[(&str, &str)],
    effective: Option<&str>,
    real: Option<&str>,
) -> WithPasswd<FixedEnvironment> {
    WithPasswd {
        environment: FixedEnvironment {
            directory: PathBuf::from("/work"),
            variables: variables
                .iter()
                .map(|(key, value)| (OsString::from(key), OsString::from(value)))
                .collect::<BTreeMap<_, _>>(),
        },
        effective: effective.map(OsString::from),
        real: real.map(OsString::from),
    }
}

fn posix_environment(home: &Value) -> WithPasswd<FixedEnvironment> {
    let home = home.as_str().map(str::to_owned);
    let variables: Vec<(&str, &str)> = home.as_deref().map(|home| ("HOME", home)).into_iter().collect();
    environment(&variables, Some(PASSWD_HOME), Some(PASSWD_HOME))
}

#[test]
fn node_posix_matches_the_real_runtime() {
    let golden = golden();
    for row in golden["node_posix"].as_array().expect("rows") {
        let environment = posix_environment(&row["env_home"]);
        let label = row["env_home"].to_string();
        let home = node_homedir(&environment, Platform::Posix).expect("posix home");
        assert_eq!(home, OsString::from(text(&row["homedir"])), "homedir for {label}");
        let claude = node_join(Platform::Posix, &home, ".claude");
        assert_eq!(claude, PathBuf::from(text(&row["claude"])), "claude for {label}");
        assert_eq!(
            node_join(Platform::Posix, &home, ".claude.json"),
            PathBuf::from(text(&row["claudeJson"])),
            "claude.json for {label}"
        );
        assert_eq!(
            node_join(Platform::Posix, claude.as_os_str(), "settings.json"),
            PathBuf::from(text(&row["settings"])),
            "settings for {label}"
        );
    }
}

#[test]
fn python_posix_matches_the_real_runtime() {
    let golden = golden();
    for row in golden["python_posix"].as_array().expect("rows") {
        let environment = posix_environment(&row["env_home"]);
        let label = row["env_home"].to_string();
        let home = python_home(&environment, Platform::Posix);
        if let Some(message) = row.get("error") {
            let error = home.expect_err("a home starting with a tilde is refused");
            assert_eq!(error.message(), message.as_str().expect("message"), "error for {label}");
            assert_eq!(error.to_string(), error.message());
            continue;
        }
        let home = home.expect("python home");
        assert_eq!(
            python_join(Platform::Posix, &home, ".claude"),
            PathBuf::from(text(&row["claude"])),
            "claude for {label}"
        );
        assert_eq!(
            python_join(Platform::Posix, &home, ".claude.json"),
            PathBuf::from(text(&row["claude_json"])),
            "claude.json for {label}"
        );
        assert_eq!(
            posix_expanduser(&environment),
            Some(OsString::from(text(&row["expanduser"]))),
            "expanduser for {label}"
        );
    }
}

#[test]
fn python_windows_matches_the_real_ntpath_and_pathlib() {
    let golden = golden();
    for row in golden["python_windows"].as_array().expect("rows") {
        let variables: Vec<(String, String)> = row["env"]
            .as_object()
            .expect("env")
            .iter()
            .map(|(key, value)| (key.clone(), value.as_str().expect("value").to_owned()))
            .collect();
        let borrowed: Vec<(&str, &str)> = variables.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let environment = environment(&borrowed, Some(PASSWD_HOME), Some(PASSWD_HOME));
        let home = python_home(&environment, Platform::Windows);
        let label = row["env"].to_string();
        if row["failed"].as_bool().expect("failed") {
            assert_eq!(home, Err(HomeDirectoryError::PYTHON), "{label}");
            continue;
        }
        let home = home.expect("python home");
        assert_eq!(
            home,
            PathBuf::from(row["home"].as_str().expect("home")),
            "home for {label}"
        );
        assert_eq!(
            python_join(Platform::Windows, &home, ".claude"),
            PathBuf::from(row["claude"].as_str().expect("claude")),
            "claude for {label}"
        );
        assert_eq!(
            python_join(Platform::Windows, &home, ".local/bin/claude.exe"),
            PathBuf::from(row["local_bin"].as_str().expect("local_bin")),
            "local bin for {label}"
        );
    }
}

#[test]
fn pathlib_windows_joins_match_the_real_pure_windows_path() {
    let golden = golden();
    for row in golden["python_nt_join"].as_array().expect("rows") {
        let home = row["home"].as_str().expect("home");
        let segment = row["segment"].as_str().expect("segment");
        let home_str = nt_path::normalize(home);
        assert_eq!(home_str, row["home_str"].as_str().expect("home_str"), "home {home:?}");
        assert_eq!(
            python_join(Platform::Windows, Path::new(&home_str), segment),
            PathBuf::from(row["result"].as_str().expect("result")),
            "{home:?} / {segment:?}"
        );
    }
}

#[test]
fn node_windows_joins_match_the_real_path_win32_join() {
    let golden = golden();
    for row in golden["node_win32_join"].as_array().expect("rows") {
        let base = row["base"].as_str().expect("base");
        let segment = row["segment"].as_str().expect("segment");
        assert_eq!(
            node_join(Platform::Windows, OsStr::new(base), segment),
            PathBuf::from(row["result"].as_str().expect("result")),
            "join({base:?}, {segment:?})"
        );
    }
}

#[test]
fn a_user_without_a_passwd_entry_fails_like_the_real_runtimes() {
    let golden = golden();
    let environment = environment(&[], None, None);
    let node = node_homedir(&environment, Platform::Posix).expect_err("no passwd entry");
    assert_eq!(
        node.message(),
        golden["no_passwd_entry"]["node"]["error"].as_str().expect("node")
    );
    let python = python_home(&environment, Platform::Posix).expect_err("no passwd entry");
    assert_eq!(
        python.message(),
        golden["no_passwd_entry"]["python"]["error"].as_str().expect("python")
    );
}

#[test]
fn node_follows_the_effective_user_and_python_the_real_user() {
    let golden = golden();
    let node_error = golden["split_uids"]["node"]["error"].as_str().expect("node error");
    let python_home_text = text(&golden["split_uids"]["python"]["home"]);

    let split = environment(&[], None, Some(PASSWD_HOME));
    assert_eq!(
        node_homedir(&split, Platform::Posix)
            .expect_err("effective user unknown")
            .message(),
        node_error
    );
    assert_eq!(
        python_home(&split, Platform::Posix),
        Ok(PathBuf::from(python_home_text))
    );

    let reverse = environment(&[], Some(PASSWD_HOME), None);
    assert_eq!(node_homedir(&reverse, Platform::Posix), Ok(OsString::from(PASSWD_HOME)));
    assert_eq!(python_home(&reverse, Platform::Posix), Err(HomeDirectoryError::PYTHON));
}

#[test]
fn node_windows_reads_userprofile_and_rejects_values_shorter_than_three_bytes() {
    let host = |variables: &[(&str, &str)], effective| environment(variables, effective, None);
    assert_eq!(
        node_homedir(&host(&[("USERPROFILE", "C:\\Users\\a")], None), Platform::Windows),
        Ok(OsString::from("C:\\Users\\a"))
    );
    assert_eq!(
        node_homedir(&host(&[("USERPROFILE", "abc")], None), Platform::Windows),
        Ok(OsString::from("abc"))
    );
    for short in ["", "a", "ab"] {
        assert_eq!(
            node_homedir(&host(&[("USERPROFILE", short)], Some(PASSWD_HOME)), Platform::Windows),
            Err(HomeDirectoryError::NODE),
            "USERPROFILE={short:?}"
        );
    }
    assert_eq!(
        node_homedir(&host(&[("HOME", "/ignored")], Some(PASSWD_HOME)), Platform::Windows),
        Ok(OsString::from(PASSWD_HOME))
    );
    assert_eq!(
        node_homedir(&host(&[], None), Platform::Windows),
        Err(HomeDirectoryError::NODE)
    );
}

#[test]
fn node_join_normalizes_posix_paths_like_path_join() {
    let join = |base: &str, segment: &str| node_join(Platform::Posix, OsStr::new(base), segment);
    assert_eq!(join("", ".claude"), PathBuf::from(".claude"));
    assert_eq!(join("", "/x"), PathBuf::from("/x"));
    assert_eq!(join("/", ".."), PathBuf::from("/"));
    assert_eq!(join("/..", ".claude"), PathBuf::from("/.claude"));
    assert_eq!(join("../..", ".claude"), PathBuf::from("../../.claude"));
    assert_eq!(join("a/b", ".."), PathBuf::from("a"));
}

#[test]
fn the_default_passwd_methods_ask_the_operating_system() {
    let system = crate::adapters::SystemEnvironment;
    // The test process runs with equal real and effective user ids, so both lookups read one entry.
    assert_eq!(
        system.effective_user_home_directory(),
        system.real_user_home_directory()
    );
}

#[cfg(unix)]
fn from_hex(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).expect("hex byte"))
        .collect()
}

#[cfg(unix)]
fn raw_home_environment(raw: &[u8]) -> WithPasswd<FixedEnvironment> {
    use std::os::unix::ffi::OsStringExt;

    WithPasswd {
        environment: FixedEnvironment {
            directory: PathBuf::from("/work"),
            variables: BTreeMap::from([(OsString::from("HOME"), OsString::from_vec(raw.to_vec()))]),
        },
        effective: None,
        real: None,
    }
}

#[cfg(unix)]
#[test]
fn node_decodes_a_home_that_is_not_utf8_as_lossy_utf8_before_joining() {
    let golden = golden();
    for row in golden["node_posix_bytes"].as_array().expect("rows") {
        let raw = from_hex(row["env_home_hex"].as_str().expect("hex"));
        let environment = raw_home_environment(&raw);
        let home = node_homedir(&environment, Platform::Posix).expect("home");
        let label = row["env_home_hex"].to_string();
        assert_eq!(
            home.to_string_lossy(),
            row["homedir"].as_str().expect("homedir"),
            "{label}"
        );
        assert_eq!(
            node_join(Platform::Posix, &home, ".claude"),
            PathBuf::from(row["claude"].as_str().expect("claude")),
            "claude for {label}"
        );
        assert_eq!(
            node_join(Platform::Posix, &home, ".claude.json"),
            PathBuf::from(row["claudeJson"].as_str().expect("claudeJson")),
            "claude.json for {label}"
        );
    }
}

#[cfg(unix)]
#[test]
fn python_keeps_the_bytes_of_a_home_that_is_not_utf8() {
    use std::os::unix::ffi::OsStrExt;

    let golden = golden();
    for row in golden["python_posix_bytes"].as_array().expect("rows") {
        let raw = from_hex(row["env_home_hex"].as_str().expect("hex"));
        let environment = raw_home_environment(&raw);
        let home = python_home(&environment, Platform::Posix);
        let label = row["env_home_hex"].to_string();
        if let Some(message) = row.get("error") {
            assert_eq!(
                home.expect_err("refused").message(),
                message.as_str().expect("message"),
                "{label}"
            );
            continue;
        }
        let home = home.expect("home");
        let bytes = |key: &str| from_hex(row[key].as_str().expect("hex"));
        assert_eq!(home.as_os_str().as_bytes(), bytes("home"), "home for {label}");
        assert_eq!(
            python_join(Platform::Posix, &home, ".claude").as_os_str().as_bytes(),
            bytes("claude"),
            "claude for {label}"
        );
        assert_eq!(
            python_join(Platform::Posix, &home, ".claude.json")
                .as_os_str()
                .as_bytes(),
            bytes("claude_json"),
            "claude.json for {label}"
        );
    }
}
