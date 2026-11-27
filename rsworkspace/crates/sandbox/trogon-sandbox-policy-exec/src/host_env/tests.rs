use super::*;

fn pair(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

#[test]
fn the_marker_is_added_and_nothing_is_stripped() {
    let env = merged(vec![pair("PATH", "/bin"), pair("SECRET_TOKEN", "x")], &[]);
    assert_eq!(
        env,
        vec![
            pair("PATH", "/bin"),
            pair("SECRET_TOKEN", "x"),
            pair("TROGON_SANDBOXED", "1")
        ]
    );
}

#[test]
fn later_layers_override_earlier_ones() {
    let config = [pair("PATH", "/config"), pair("A", "1")];
    let spec = [pair("A", "2")];
    let env = merged(vec![pair("PATH", "/bin")], &[&config, &spec]);
    assert_eq!(
        env,
        vec![pair("PATH", "/config"), pair("TROGON_SANDBOXED", "1"), pair("A", "2")]
    );
}

#[test]
fn the_tmpdir_is_the_current_name_then_the_legacy_one_then_the_default() {
    assert_eq!(tmpdir(Some("/a"), None), "/a");
    assert_eq!(tmpdir(None, Some("/b")), "/b");
    assert_eq!(tmpdir(Some("/a"), Some("/b")), "/a");
    assert_eq!(tmpdir(None, None), "/tmp/sandbox");
    assert_eq!(tmpdir(Some(""), Some("/b")), "/b");
    assert_eq!(tmpdir(Some(""), Some("")), "/tmp/sandbox");
}

#[test]
fn the_guest_ca_path_becomes_the_host_file() {
    let vars = vec![pair("SSL_CERT_FILE", "/guest/ca.pem"), pair("OTHER", "x")];
    let fixed = with_host_ca(vars, "/guest/ca.pem", Some(Path::new("/host/ca.pem")));
    assert_eq!(fixed[0].1, "/host/ca.pem");
    assert_eq!(fixed[1].1, "x");
}
