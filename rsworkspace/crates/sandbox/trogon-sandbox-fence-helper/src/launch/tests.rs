use super::*;

#[test]
fn quote_arg_simple() {
    assert_eq!(quote_arg("foo"), "foo");
    assert_eq!(quote_arg(""), "\"\"");
    assert_eq!(quote_arg("a b"), "\"a b\"");
}

#[test]
fn quote_arg_backslash_quote() {
    // a\"b → "a\\\"b"
    assert_eq!(quote_arg(r#"a\"b"#), r#""a\\\"b""#);
    // trailing backslashes double before closing quote
    assert_eq!(quote_arg(r"a\"), r#""a\\""#);
    assert_eq!(quote_arg(r"a\\"), r#""a\\\\""#);
}

#[test]
fn build_cmdline_cmd_passthrough() {
    let exe = Path::new(r"C:\Windows\System32\cmd.exe");
    // post-/c content is wrapped once in "…" for /s to strip;
    // inner quotes and metachars are NOT touched.
    let line = build_cmdline(exe, &["/d".into(), "/s".into(), "/c".into(), r#"echo "x & y""#.into()]);
    assert_eq!(line, r#""C:\Windows\System32\cmd.exe" /d /s /c "echo "x & y"""#);
    // Multiple post-/c argv elements are joined with a space.
    let line2 = build_cmdline(
        exe,
        &[
            "/c".into(),
            "echo".into(),
            "a".into(),
            "&".into(),
            "echo".into(),
            "b".into(),
        ],
    );
    assert_eq!(line2, r#""C:\Windows\System32\cmd.exe" /c "echo a & echo b""#);
}

#[test]
fn build_cmdline_cmd_no_split_when_no_c_flag() {
    // cmd.exe without /c|/k|/r → MSVCRT quoting throughout.
    let exe = Path::new("cmd.exe");
    let line = build_cmdline(exe, &["/?".into()]);
    assert_eq!(line, r#"cmd.exe /?"#);
}

#[test]
fn build_cmdline_non_cmd_uses_msvcrt_quoting() {
    let exe = Path::new(r"C:\foo\bar.exe");
    let args = vec![r#"a "b"#.into()];
    let line = build_cmdline(exe, &args);
    assert!(line.ends_with(r#""a \"b""#), "got: {line}");
}

fn os(pairs: &[(&str, &str)]) -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
    pairs.iter().map(|(k, v)| (k.into(), v.into())).collect()
}

fn overlay(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

/// The merged entries, checked to hold no two names equal ignoring case.
fn merged(inherited: &[(&str, &str)], over: &[(&str, &str)]) -> Vec<(String, String)> {
    let entries: Vec<(String, String)> = merge_env(os(inherited), &overlay(over))
        .into_iter()
        .map(|(k, v)| (k.to_string_lossy().into_owned(), v.to_string_lossy().into_owned()))
        .collect();
    let mut seen = std::collections::HashSet::new();
    for (k, _) in &entries {
        assert!(
            seen.insert(k.to_uppercase()),
            "{k} is in the block twice ignoring case: {entries:?}"
        );
    }
    entries
}

fn value_of<'a>(entries: &'a [(String, String)], name: &str) -> Option<&'a str> {
    entries.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

#[test]
fn env_overlay_with_both_spellings_yields_one_name() {
    let entries = merged(
        &[("SystemRoot", r"C:\Windows")],
        &[
            ("HTTP_PROXY", "http://localhost:60080"),
            ("HTTPS_PROXY", "http://localhost:60080"),
            ("http_proxy", "http://localhost:60080"),
            ("https_proxy", "http://localhost:60080"),
            ("ALL_PROXY", "http://localhost:60080"),
            ("all_proxy", "http://localhost:60080"),
            ("GRPC_PROXY", "http://localhost:60080"),
            ("grpc_proxy", "http://localhost:60080"),
            ("FTP_PROXY", "socks5h://localhost:60081"),
            ("ftp_proxy", "socks5h://localhost:60081"),
            ("RSYNC_PROXY", "localhost:60081"),
        ],
    );
    assert_eq!(entries.len(), 7, "{entries:?}");
    for name in ["http_proxy", "https_proxy", "all_proxy", "grpc_proxy"] {
        assert_eq!(value_of(&entries, name), Some("http://localhost:60080"));
    }
    assert_eq!(value_of(&entries, "ftp_proxy"), Some("socks5h://localhost:60081"));
    assert_eq!(value_of(&entries, "RSYNC_PROXY"), Some("localhost:60081"));
}

#[test]
fn env_proxy_names_have_one_spelling_whatever_was_given() {
    for (given, spelled) in [
        ("HTTP_PROXY", "http_proxy"),
        ("Http_Proxy", "http_proxy"),
        ("http_proxy", "http_proxy"),
        ("HTTPS_PROXY", "https_proxy"),
        ("FTP_PROXY", "ftp_proxy"),
        ("ALL_PROXY", "all_proxy"),
        ("NO_PROXY", "no_proxy"),
        ("GRPC_PROXY", "grpc_proxy"),
        ("rsync_proxy", "RSYNC_PROXY"),
        ("RSYNC_PROXY", "RSYNC_PROXY"),
    ] {
        let entries = merged(&[], &[(given, "v")]);
        assert_eq!(entries, vec![(spelled.to_string(), "v".to_string())]);
        let entries = merged(&[(given, "v")], &[]);
        assert_eq!(entries, vec![(spelled.to_string(), "v".to_string())]);
    }
}

#[test]
fn env_other_names_keep_their_spelling_and_get_no_second_one() {
    let over = [
        ("NODE_USE_ENV_PROXY", "1"),
        ("DOCKER_HTTP_PROXY", "http://localhost:60080"),
        ("Foo_Proxy", "x"),
        ("HTTP_PROXYX", "x"),
        ("PATH", r"C:\Windows"),
    ];
    let entries = merged(&[], &over);
    assert_eq!(entries.len(), over.len(), "{entries:?}");
    for (name, value) in over {
        assert_eq!(value_of(&entries, name), Some(value));
    }
}

#[test]
fn env_overlay_replaces_an_inherited_name_in_any_case() {
    let entries = merged(
        &[
            ("Path", r"C:\Users\sandbox\bin"),
            ("HTTPS_PROXY", "http://inherited:1"),
            ("node_use_env_proxy", "1"),
            ("TEMP", r"C:\Users\sandbox\Temp"),
        ],
        &[
            ("PATH", r"C:\Windows"),
            ("https_proxy", "http://localhost:60080"),
            ("NODE_USE_ENV_PROXY", "0"),
        ],
    );
    assert_eq!(entries.len(), 4, "{entries:?}");
    assert_eq!(value_of(&entries, "PATH"), Some(r"C:\Windows"));
    assert_eq!(value_of(&entries, "https_proxy"), Some("http://localhost:60080"));
    assert_eq!(value_of(&entries, "NODE_USE_ENV_PROXY"), Some("0"));
    assert_eq!(value_of(&entries, "TEMP"), Some(r"C:\Users\sandbox\Temp"));
}

#[test]
fn env_inherited_names_that_differ_by_case_yield_the_first() {
    let entries = merged(
        &[
            ("HTTP_PROXY", "http://first:1"),
            ("http_proxy", "http://second:2"),
            ("Foo", "first"),
            ("FOO", "second"),
        ],
        &[],
    );
    assert_eq!(
        entries,
        vec![
            ("Foo".to_string(), "first".to_string()),
            ("http_proxy".to_string(), "http://first:1".to_string()),
        ]
    );
}

#[test]
fn env_last_overlay_entry_for_a_name_wins() {
    let entries = merged(
        &[],
        &[
            ("NODE_USE_ENV_PROXY", "1"),
            ("node_use_env_proxy", ""),
            ("HTTPS_PROXY", "http://masked"),
            ("https_proxy", "http://localhost:60080"),
        ],
    );
    assert_eq!(
        entries,
        vec![
            ("https_proxy".to_string(), "http://localhost:60080".to_string()),
            ("node_use_env_proxy".to_string(), String::new()),
        ]
    );
}

#[test]
fn env_names_fold_beyond_ascii_and_block_is_sorted() {
    let entries = merged(&[("é", "1"), ("b", "2"), ("A", "3")], &[("É", "4")]);
    assert_eq!(
        entries,
        vec![
            ("A".to_string(), "3".to_string()),
            ("b".to_string(), "2".to_string()),
            ("É".to_string(), "4".to_string()),
        ]
    );
}

#[test]
fn env_block_is_name_value_pairs_with_a_final_nul() {
    let block = encode_env_block(os(&[("A", "1"), ("b", "x=y")]));
    let expected: Vec<u16> = "A=1\0b=x=y\0\0".encode_utf16().collect();
    assert_eq!(block, expected);
}
