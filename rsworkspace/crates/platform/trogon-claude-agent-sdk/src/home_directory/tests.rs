#[cfg(unix)]
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::{collections::BTreeMap, ffi::OsString, path::PathBuf};

use serde_json::Value;

use super::*;
use crate::test_support::{FixedEnvironment, WithPasswd};

const GOLDEN: &str = include_str!("../../tests/fixtures/home_directory/golden.json");
const PLACEHOLDER: &str = "<PASSWD_HOME>";
const PASSWD_HOME: &str = "/passwd/home";

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn golden() -> serde_json::Result<Value> {
    serde_json::from_str(GOLDEN)
}

fn string<'a>(value: &'a Value, what: &str) -> Result<&'a str, String> {
    value
        .as_str()
        .ok_or_else(|| format!("{what} is not a string in the golden fixture"))
}

fn rows<'a>(golden: &'a Value, key: &str) -> Result<&'a Vec<Value>, String> {
    golden[key]
        .as_array()
        .ok_or_else(|| format!("{key} has no rows in the golden fixture"))
}

fn text(value: &Value) -> Result<String, String> {
    Ok(string(value, "a golden path")?.replace(PLACEHOLDER, PASSWD_HOME))
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
fn node_posix_matches_the_real_runtime() -> TestResult {
    let golden = golden()?;
    for row in rows(&golden, "node_posix")? {
        let environment = posix_environment(&row["env_home"]);
        let label = row["env_home"].to_string();
        let home = node_homedir(&environment, Platform::Posix)?;
        assert_eq!(home, OsString::from(text(&row["homedir"])?), "homedir for {label}");
        let claude = node_join(Platform::Posix, &home, ".claude");
        assert_eq!(claude, PathBuf::from(text(&row["claude"])?), "claude for {label}");
        assert_eq!(
            node_join(Platform::Posix, &home, ".claude.json"),
            PathBuf::from(text(&row["claudeJson"])?),
            "claude.json for {label}"
        );
        assert_eq!(
            node_join(Platform::Posix, claude.as_os_str(), "settings.json"),
            PathBuf::from(text(&row["settings"])?),
            "settings for {label}"
        );
    }
    Ok(())
}

#[test]
fn python_posix_matches_the_real_runtime() -> TestResult {
    let golden = golden()?;
    for row in rows(&golden, "python_posix")? {
        let environment = posix_environment(&row["env_home"]);
        let label = row["env_home"].to_string();
        let home = python_home(&environment, Platform::Posix);
        if let Some(message) = row.get("error") {
            let Err(error) = home else {
                return Err(format!("a home starting with a tilde was accepted for {label}").into());
            };
            assert_eq!(error.message(), string(message, "message")?, "error for {label}");
            assert_eq!(error.to_string(), error.message());
            continue;
        }
        let home = home?;
        assert_eq!(
            python_join(Platform::Posix, &home, ".claude"),
            PathBuf::from(text(&row["claude"])?),
            "claude for {label}"
        );
        assert_eq!(
            python_join(Platform::Posix, &home, ".claude.json"),
            PathBuf::from(text(&row["claude_json"])?),
            "claude.json for {label}"
        );
        assert_eq!(
            posix_expanduser(&environment),
            Some(OsString::from(text(&row["expanduser"])?)),
            "expanduser for {label}"
        );
    }
    Ok(())
}

#[test]
fn python_windows_matches_the_real_ntpath_and_pathlib() -> TestResult {
    let golden = golden()?;
    for row in rows(&golden, "python_windows")? {
        let variables: Vec<(String, String)> = row["env"]
            .as_object()
            .ok_or("env is not an object in the golden fixture")?
            .iter()
            .map(|(key, value)| -> Result<(String, String), String> {
                Ok((key.clone(), string(value, key)?.to_owned()))
            })
            .collect::<Result<_, String>>()?;
        let borrowed: Vec<(&str, &str)> = variables.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let environment = environment(&borrowed, Some(PASSWD_HOME), Some(PASSWD_HOME));
        let home = python_home(&environment, Platform::Windows);
        let label = row["env"].to_string();
        if row["failed"]
            .as_bool()
            .ok_or("failed is not a boolean in the golden fixture")?
        {
            assert_eq!(home, Err(HomeDirectoryError::PYTHON), "{label}");
            continue;
        }
        let home = home?;
        assert_eq!(home, PathBuf::from(string(&row["home"], "home")?), "home for {label}");
        assert_eq!(
            python_join(Platform::Windows, &home, ".claude"),
            PathBuf::from(string(&row["claude"], "claude")?),
            "claude for {label}"
        );
        assert_eq!(
            python_join(Platform::Windows, &home, ".local/bin/claude.exe"),
            PathBuf::from(string(&row["local_bin"], "local_bin")?),
            "local bin for {label}"
        );
    }
    Ok(())
}

#[test]
fn pathlib_windows_joins_match_the_real_pure_windows_path() -> TestResult {
    let golden = golden()?;
    for row in rows(&golden, "python_nt_join")? {
        let home = string(&row["home"], "home")?;
        let segment = string(&row["segment"], "segment")?;
        let home_str = nt_path::normalize(home);
        assert_eq!(home_str, string(&row["home_str"], "home_str")?, "home {home:?}");
        assert_eq!(
            python_join(Platform::Windows, Path::new(&home_str), segment),
            PathBuf::from(string(&row["result"], "result")?),
            "{home:?} / {segment:?}"
        );
    }
    Ok(())
}

#[test]
fn node_windows_joins_match_the_real_path_win32_join() -> TestResult {
    let golden = golden()?;
    for row in rows(&golden, "node_win32_join")? {
        let base = string(&row["base"], "base")?;
        let segment = string(&row["segment"], "segment")?;
        assert_eq!(
            node_join(Platform::Windows, OsStr::new(base), segment),
            PathBuf::from(string(&row["result"], "result")?),
            "join({base:?}, {segment:?})"
        );
    }
    Ok(())
}

#[test]
fn a_user_without_a_passwd_entry_fails_like_the_real_runtimes() -> TestResult {
    let golden = golden()?;
    let environment = environment(&[], None, None);
    let Err(node) = node_homedir(&environment, Platform::Posix) else {
        return Err("node found a home without a passwd entry".into());
    };
    assert_eq!(
        node.message(),
        string(&golden["no_passwd_entry"]["node"]["error"], "node")?
    );
    let Err(python) = python_home(&environment, Platform::Posix) else {
        return Err("python found a home without a passwd entry".into());
    };
    assert_eq!(
        python.message(),
        string(&golden["no_passwd_entry"]["python"]["error"], "python")?
    );
    Ok(())
}

#[test]
fn node_follows_the_effective_user_and_python_the_real_user() -> TestResult {
    let golden = golden()?;
    let node_error = string(&golden["split_uids"]["node"]["error"], "node error")?;
    let python_home_text = text(&golden["split_uids"]["python"]["home"])?;

    let split = environment(&[], None, Some(PASSWD_HOME));
    let Err(unknown) = node_homedir(&split, Platform::Posix) else {
        return Err("node found a home for an unknown effective user".into());
    };
    assert_eq!(unknown.message(), node_error);
    assert_eq!(
        python_home(&split, Platform::Posix),
        Ok(PathBuf::from(python_home_text))
    );

    let reverse = environment(&[], Some(PASSWD_HOME), None);
    assert_eq!(node_homedir(&reverse, Platform::Posix), Ok(OsString::from(PASSWD_HOME)));
    assert_eq!(python_home(&reverse, Platform::Posix), Err(HomeDirectoryError::PYTHON));
    Ok(())
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
fn from_hex(hex: &str) -> Result<Vec<u8>, std::num::ParseIntError> {
    (0..hex.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&hex[index..index + 2], 16))
        .collect()
}

#[cfg(unix)]
fn raw_home_environment(raw: &[u8]) -> WithPasswd<FixedEnvironment> {
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
fn node_decodes_a_home_that_is_not_utf8_as_lossy_utf8_before_joining() -> TestResult {
    let golden = golden()?;
    for row in rows(&golden, "node_posix_bytes")? {
        let raw = from_hex(string(&row["env_home_hex"], "hex")?)?;
        let environment = raw_home_environment(&raw);
        let home = node_homedir(&environment, Platform::Posix)?;
        let label = row["env_home_hex"].to_string();
        assert_eq!(home.to_string_lossy(), string(&row["homedir"], "homedir")?, "{label}");
        assert_eq!(
            node_join(Platform::Posix, &home, ".claude"),
            PathBuf::from(string(&row["claude"], "claude")?),
            "claude for {label}"
        );
        assert_eq!(
            node_join(Platform::Posix, &home, ".claude.json"),
            PathBuf::from(string(&row["claudeJson"], "claudeJson")?),
            "claude.json for {label}"
        );
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn python_keeps_the_bytes_of_a_home_that_is_not_utf8() -> TestResult {
    let golden = golden()?;
    for row in rows(&golden, "python_posix_bytes")? {
        let raw = from_hex(string(&row["env_home_hex"], "hex")?)?;
        let environment = raw_home_environment(&raw);
        let home = python_home(&environment, Platform::Posix);
        let label = row["env_home_hex"].to_string();
        if let Some(message) = row.get("error") {
            let Err(error) = home else {
                return Err(format!("a home that is not UTF-8 was accepted for {label}").into());
            };
            assert_eq!(error.message(), string(message, "message")?, "{label}");
            continue;
        }
        let home = home?;
        let bytes =
            |key: &str| -> Result<Vec<u8>, Box<dyn std::error::Error>> { Ok(from_hex(string(&row[key], key)?)?) };
        assert_eq!(home.as_os_str().as_bytes(), bytes("home")?, "home for {label}");
        assert_eq!(
            python_join(Platform::Posix, &home, ".claude").as_os_str().as_bytes(),
            bytes("claude")?,
            "claude for {label}"
        );
        assert_eq!(
            python_join(Platform::Posix, &home, ".claude.json")
                .as_os_str()
                .as_bytes(),
            bytes("claude_json")?,
            "claude.json for {label}"
        );
    }
    Ok(())
}
