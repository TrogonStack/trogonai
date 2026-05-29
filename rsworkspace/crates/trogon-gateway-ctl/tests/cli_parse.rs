use std::path::PathBuf;
use std::process::Command;

fn target_binary() -> PathBuf {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_trogon-gateway-ctl") {
        return PathBuf::from(path);
    }

    // Integration tests run from target/debug/deps/; the binary sits in target/debug/.
    let current = std::env::current_exe().expect("current test executable");
    current
        .parent()
        .and_then(|deps| deps.parent())
        .map(|debug| debug.join("trogon-gateway-ctl"))
        .filter(|path| path.exists())
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/debug/trogon-gateway-ctl")
        })
}

fn bin() -> Command {
    let path = target_binary();
    assert!(
        path.exists(),
        "trogon-gateway-ctl binary not found at {}; run `cargo build -p trogon-gateway-ctl`",
        path.display()
    );
    Command::new(path)
}

#[test]
fn help_lists_core_verbs() {
    let output = bin().arg("--help").output().expect("help");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("config"));
    assert!(stdout.contains("trace"));
    assert!(stdout.contains("bundle"));
    assert!(stdout.contains("policy"));
    assert!(stdout.contains("audit"));
}

#[test]
fn missing_subcommand_is_usage_error() {
    let output = bin().output().expect("no args");
    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(3));
}
