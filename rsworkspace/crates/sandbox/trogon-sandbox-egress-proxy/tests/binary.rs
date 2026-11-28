use std::error::Error;
use std::process::Command;

type TestResult = Result<(), Box<dyn Error>>;

fn proxy() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_trogon-sandbox-egress-proxyd"));
    command.env_clear();
    command
}

#[test]
fn an_unknown_flag_stops_the_process_with_a_failure() -> TestResult {
    let output = proxy().arg("--no-such-flag").output()?;

    assert_eq!(output.status.code(), Some(1));
    Ok(())
}

#[test]
fn conflicting_listen_flags_stop_the_process_with_a_failure() -> TestResult {
    let output = proxy().args(["--listen", "127.0.0.1:0", "--listen-fd", "7"]).output()?;

    assert_eq!(output.status.code(), Some(1));
    Ok(())
}

#[test]
fn a_process_without_any_configuration_stops_with_a_failure() -> TestResult {
    let output = proxy().output()?;

    assert_eq!(output.status.code(), Some(1));
    Ok(())
}

#[test]
fn a_non_loopback_listen_address_stops_the_process_unless_opted_in() -> TestResult {
    let policy = std::env::temp_dir().join(format!("egress-proxy-binary-policy-{}.json", std::process::id()));
    std::fs::write(&policy, r#""deny_all""#)?;
    let refused = proxy()
        .env("EGRESS_PROXY_POLICY_FILE", &policy)
        .env("EGRESS_PROXY_LISTEN_ADDR", "0.0.0.0:0")
        .output()?;
    let refused_flag = proxy()
        .env("EGRESS_PROXY_POLICY_FILE", &policy)
        .env("EGRESS_PROXY_MUX_TOKEN", "token")
        .env("EGRESS_PROXY_MUX_LISTEN_ADDR", "127.0.0.1:0")
        .env("EGRESS_PROXY_LISTEN_ADDR", "off")
        .args(["--listen", "0.0.0.0:0"])
        .output()?;
    std::fs::remove_file(&policy)?;

    for output in [refused, refused_flag] {
        assert_eq!(output.status.code(), Some(1));
        let logged = String::from_utf8_lossy(&output.stdout).into_owned() + &String::from_utf8_lossy(&output.stderr);
        assert!(logged.contains("--allow-non-loopback-listen"), "{logged}");
    }
    Ok(())
}
