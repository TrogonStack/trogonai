use trogon_sandbox_windows_fence::{
    BinShell, ExecRequest, WINDOWS_LOOPBACK_NO_PROXY, WindowsSandboxErrorCode as Code, merge_windows_env,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const ROOT: &str = "C:\\Windows";

fn kv(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn cmd() -> Result<BinShell, Box<dyn std::error::Error>> {
    Ok(BinShell::parse(None, ROOT)?)
}

#[test]
fn bin_shell_defaults_and_tokens() -> TestResult {
    assert_eq!(
        cmd()?,
        BinShell {
            exe: "C:\\Windows\\System32\\cmd.exe".to_string(),
            args: ["/d", "/s", "/c"].map(String::from).to_vec()
        }
    );
    assert_eq!(BinShell::parse(Some("CMD"), ROOT)?, cmd()?);
    assert_eq!(BinShell::parse(Some("pwsh"), ROOT)?.exe, "pwsh.exe");
    let ps = BinShell::parse(Some("powershell"), "C:\\Windows\\")?;
    assert_eq!(ps.exe, "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe");
    assert_eq!(ps.args, ["-NoProfile", "-Command"]);
    Ok(())
}

#[test]
fn bin_shell_absolute_paths_keep_their_flags() -> TestResult {
    let bash = BinShell::parse(Some("C:\\Git\\bin\\bash.exe"), ROOT)?;
    assert_eq!(
        (bash.exe.as_str(), bash.args.as_slice()),
        ("C:\\Git\\bin\\bash.exe", ["-c".to_string()].as_slice())
    );
    let sh = BinShell::parse(Some("C:/Git/usr/bin/SH.EXE"), ROOT)?;
    assert_eq!(sh.args, ["-c"]);
    let c = BinShell::parse(Some("D:\\x\\cmd.exe"), ROOT)?;
    assert_eq!((c.exe.as_str(), c.args.len()), ("D:\\x\\cmd.exe", 3));
    let p = BinShell::parse(Some("D:\\ps\\pwsh.exe"), ROOT)?;
    assert_eq!(p.exe, "D:\\ps\\pwsh.exe");
    Ok(())
}

#[test]
fn bin_shell_rejects_unsupported_shells() -> TestResult {
    for bad in [
        "bash",
        "sh.exe",
        "bin\\bash.exe",
        "zsh",
        "C:\\x\\fish.exe",
        "C:bash.exe",
    ] {
        let Err(e) = BinShell::parse(Some(bad), ROOT) else {
            return Err(format!("{bad} should be rejected").into());
        };
        assert_eq!(e.code(), Code::BinShellInvalid, "{bad}");
    }
    let Err(e) = BinShell::explicit("relative.exe", vec![]) else {
        return Err("expected an error".into());
    };
    assert_eq!(e.code(), Code::BinShellInvalid);
    assert!(BinShell::explicit("C:\\a.exe", vec!["x".to_string()]).is_ok());
    Ok(())
}

#[test]
fn env_merge_ignores_case_and_normalizes_proxy_spelling() {
    let merged = merge_windows_env([
        kv(&[("Path", "a"), ("HTTPS_PROXY", "old")]).as_slice(),
        kv(&[("PATH", "b"), ("Https_Proxy", "new"), ("Other", "o")]).as_slice(),
    ]);
    assert_eq!(merged, kv(&[("PATH", "b"), ("https_proxy", "new"), ("Other", "o")]));
}

#[test]
fn argv_order_is_stable() -> TestResult {
    let mut req = ExecRequest::new("echo hi", cmd()?);
    req.held_by = Some(42);
    req.write_roots = vec!["C:\\w".to_string()];
    req.allow_loopback = true;
    req.deny_read = vec!["C:\\r".to_string()];
    req.deny_read_except = vec!["C:\\r\\ok".to_string()];
    req.deny_write = vec!["C:\\d".to_string()];
    req.base_env = kv(&[("PATH", "p")]);
    req.proxy_in_use = true;
    req.proxy_env = kv(&[
        ("HTTP_PROXY", "http://127.0.0.1:60080"),
        ("TMPDIR", "/tmp/x"),
        ("NO_PROXY", "localhost"),
        ("no_proxy", "x"),
    ]);
    req.git_env = kv(&[("GIT_CONFIG_COUNT", "1")]);
    assert_eq!(
        req.args()?,
        [
            "exec",
            "--quiet",
            "--held-by",
            "42",
            "--write-restricted-token",
            "--write-root",
            "C:\\w",
            "--allow-loopback",
            "--deny-read",
            "C:\\r",
            "--deny-read-except",
            "C:\\r\\ok",
            "--deny-write",
            "C:\\d",
            "--env",
            "PATH=p",
            "--env",
            "NODE_USE_ENV_PROXY=1",
            "--env",
            "http_proxy=http://127.0.0.1:60080",
            "--env",
            "no_proxy=localhost,127.0.0.1,::1",
            "--env",
            "GIT_CONFIG_COUNT=1",
            "--",
            "C:\\Windows\\System32\\cmd.exe",
            "/d",
            "/s",
            "/c",
            "echo hi"
        ]
    );
    Ok(())
}

#[test]
fn no_proxy_is_dropped_unless_loopback_is_open_with_a_proxy() -> TestResult {
    let mut req = ExecRequest::new("x", cmd()?);
    req.proxy_env = kv(&[("NO_PROXY", "a"), ("no_proxy", "b"), ("ALL_PROXY", "s")]);
    assert_eq!(req.env_overlay(), kv(&[("all_proxy", "s")]));
    req.allow_loopback = true;
    assert_eq!(req.env_overlay(), kv(&[("all_proxy", "s")]));
    req.proxy_in_use = true;
    assert!(
        req.env_overlay()
            .contains(&("no_proxy".to_string(), WINDOWS_LOOPBACK_NO_PROXY.to_string()))
    );
    Ok(())
}

#[test]
fn masked_proxy_names_lose_to_the_generated_set() -> TestResult {
    let mut req = ExecRequest::new("x", cmd()?);
    req.mask_env = kv(&[("HTTPS_PROXY", "masked"), ("TOKEN", "sentinel")]);
    req.proxy_env = kv(&[("https_proxy", "real")]);
    assert_eq!(req.env_overlay(), kv(&[("https_proxy", "real"), ("TOKEN", "sentinel")]));
    Ok(())
}

#[test]
fn quiet_off_and_no_overlay() -> TestResult {
    let mut req = ExecRequest::new("x", cmd()?);
    req.quiet = false;
    let a = req.args()?;
    assert_eq!(&a[..2], ["exec", "--write-restricted-token"]);
    assert!(!a.iter().any(|x| x == "--env"));
    Ok(())
}

#[test]
fn oversized_argv_is_refused_with_counts() -> TestResult {
    let mut req = ExecRequest::new("x", cmd()?);
    req.deny_write = (0..400).map(|i| format!("C:\\{}\\{i}", "d".repeat(100))).collect();
    let Err(e) = req.args() else {
        return Err("expected an error".into());
    };
    assert_eq!(e.code(), Code::ArgvTooLong);
    assert!(
        e.message()
            .contains("0 per-command read denies, 0 session grants beneath them, 400 write denies and 0 write roots")
    );
    Ok(())
}

#[test]
fn a_program_launch_passes_its_argv_without_a_command_string() -> TestResult {
    let req = ExecRequest::program("C:\\agent\\agent.exe", vec!["--serve".to_string(), "x y".to_string()])?;
    let a = req.args()?;
    assert_eq!(&a[a.len() - 4..], ["--", "C:\\agent\\agent.exe", "--serve", "x y"]);
    Ok(())
}

#[test]
fn a_program_launch_needs_an_absolute_program() {
    assert!(ExecRequest::program("agent.exe", vec![]).is_err());
}
