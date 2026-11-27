use std::path::PathBuf;

use super::*;

fn worker_env(name: &str) -> Option<String> {
    match name {
        "SystemRoot" => Some("C:\\Windows".to_string()),
        "ComSpec" => Some("C:\\Windows\\System32\\cmd.exe".to_string()),
        "NATS_CREDS" => Some("secret".to_string()),
        _ => None,
    }
}

fn get<'a>(env: &'a [(String, String)], name: &str) -> Option<&'a str> {
    env.iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

fn env(lease: &[(&str, &str)]) -> Vec<(String, String)> {
    let lease: Vec<(String, String)> = lease.iter().map(|(n, v)| (n.to_string(), v.to_string())).collect();
    sandbox_env(
        &lease,
        worker_env,
        &PathBuf::from("C:\\ws"),
        &PathBuf::from("C:\\scratch"),
    )
}

#[test]
fn only_the_allowlist_of_the_worker_environment_is_inherited() -> Result<(), Box<dyn std::error::Error>> {
    let env = env(&[]);
    assert_eq!(get(&env, "SystemRoot"), Some("C:\\Windows"));
    assert_eq!(get(&env, "ComSpec"), Some("C:\\Windows\\System32\\cmd.exe"));
    assert_eq!(get(&env, "NATS_CREDS"), None);
    Ok(())
}

#[test]
fn path_holds_the_system_directories_only() -> Result<(), Box<dyn std::error::Error>> {
    let env = env(&[]);
    assert_eq!(
        get(&env, "PATH"),
        Some("C:\\Windows\\System32;C:\\Windows;C:\\Windows\\System32\\Wbem")
    );
    Ok(())
}

#[test]
fn home_is_the_workspace_and_temp_and_app_data_are_the_scratch_directory() -> Result<(), Box<dyn std::error::Error>> {
    let env = env(&[]);
    assert_eq!(get(&env, "HOME"), Some("C:\\ws"));
    assert_eq!(get(&env, "USERPROFILE"), Some("C:\\ws"));
    for name in ["TEMP", "TMP", "TMPDIR", "LOCALAPPDATA", "APPDATA"] {
        assert_eq!(get(&env, name), Some("C:\\scratch"));
    }
    Ok(())
}

#[test]
fn the_lease_overrides_a_default_whatever_the_case_of_the_name() -> Result<(), Box<dyn std::error::Error>> {
    let env = env(&[("Path", "C:\\custom"), ("KEEP", "1")]);
    let paths: Vec<_> = env.iter().filter(|(n, _)| n.eq_ignore_ascii_case("path")).collect();
    assert_eq!(paths, [&("Path".to_string(), "C:\\custom".to_string())]);
    assert_eq!(get(&env, "KEEP"), Some("1"));
    Ok(())
}

#[test]
fn the_environment_block_is_sorted_and_double_nul_terminated() -> Result<(), Box<dyn std::error::Error>> {
    let block = environment_block(&[("b".to_string(), "2".to_string()), ("A".to_string(), "1".to_string())])?;
    let text = String::from_utf16(&block)?;
    assert_eq!(text, "A=1\0b=2\0\0");
    Ok(())
}

#[test]
fn an_empty_environment_block_is_two_nuls() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(environment_block(&[])?, vec![0, 0]);
    Ok(())
}

#[test]
fn a_name_that_would_corrupt_the_block_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    for name in ["", "A=B", "A\0"] {
        assert!(environment_block(&[(name.to_string(), "v".to_string())]).is_err());
    }
    assert!(environment_block(&[("A".to_string(), "v\0w".to_string())]).is_err());
    Ok(())
}

#[test]
fn plain_arguments_stay_bare_and_others_are_quoted() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(quote_arg("abc"), "abc");
    assert_eq!(quote_arg(""), "\"\"");
    assert_eq!(quote_arg("a b"), "\"a b\"");
    assert_eq!(quote_arg("a\"b"), "\"a\\\"b\"");
    assert_eq!(quote_arg("a\\"), "a\\");
    assert_eq!(quote_arg("a b\\"), "\"a b\\\\\"");
    assert_eq!(quote_arg("a\\\"b"), "\"a\\\\\\\"b\"");
    Ok(())
}

#[test]
fn an_ordinary_program_gets_every_argument_quoted() -> Result<(), Box<dyn std::error::Error>> {
    let line = command_line(
        Path::new("C:\\Program Files\\x\\tool.exe"),
        &["a b".to_string(), "c".to_string()],
    );
    assert_eq!(line, "\"C:\\Program Files\\x\\tool.exe\" \"a b\" c");
    Ok(())
}

#[test]
fn cmd_gets_the_command_string_whole_inside_one_pair_of_quotes() -> Result<(), Box<dyn std::error::Error>> {
    let line = command_line(
        Path::new("C:\\Windows\\System32\\cmd.exe"),
        &[
            "/d".to_string(),
            "/s".to_string(),
            "/c".to_string(),
            "echo \"a b\" & exit 3".to_string(),
        ],
    );
    assert_eq!(
        line,
        "C:\\Windows\\System32\\cmd.exe /d /s /c \"echo \"a b\" & exit 3\""
    );
    Ok(())
}

#[test]
fn cmd_without_a_command_flag_is_quoted_like_any_program() -> Result<(), Box<dyn std::error::Error>> {
    let line = command_line(Path::new("cmd.exe"), &["/d".to_string(), "x y".to_string()]);
    assert_eq!(line, "cmd.exe /d \"x y\"");
    Ok(())
}
