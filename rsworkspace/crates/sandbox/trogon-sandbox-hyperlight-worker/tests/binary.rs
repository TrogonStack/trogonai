use std::process::Command;

const BINARY: &str = env!("CARGO_BIN_EXE_trogon-sandbox-hyperlight-worker");

fn run(args: &[&str], authority: Option<&str>) -> Result<(Option<i32>, String), Box<dyn std::error::Error>> {
    let mut command = Command::new(BINARY);
    command
        .args(args)
        .env_remove("SANDBOX_WORKER_AUTHORITY")
        .env_remove("SANDBOX_HYPERLIGHT_COMPONENT_ROOT");
    if let Some(value) = authority {
        command.env("SANDBOX_WORKER_AUTHORITY", value);
    }
    let output = command.output()?;
    Ok((
        output.status.code(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    ))
}

#[test]
fn needs_a_component_root() -> Result<(), Box<dyn std::error::Error>> {
    let (code, stderr) = run(&[], None)?;
    assert_eq!(code, Some(2));
    assert!(stderr.contains("--component-root"), "{stderr}");
    Ok(())
}

#[test]
#[cfg_attr(
    all(trogon_hyperlight_wasm, any(target_os = "linux", target_os = "windows")),
    ignore = "the wasm backend serves where the host has a hypervisor"
)]
fn exits_when_the_host_has_no_hypervisor() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().to_string_lossy().into_owned();
    let (code, stderr) = run(&["--component-root", &root, "--metrics-addr", "127.0.0.1:0"], None)?;
    assert_eq!(code, Some(1));
    assert!(stderr.contains("no hypervisor"), "{stderr}");
    Ok(())
}

#[test]
fn exits_on_an_authority_mode_it_does_not_know() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().to_string_lossy().into_owned();
    let args = ["--component-root", &root, "--metrics-addr", "127.0.0.1:0"];
    let (code, stderr) = run(&args, Some("bogus"))?;
    assert_eq!(code, Some(1));
    assert!(!stderr.is_empty());
    Ok(())
}

#[test]
fn exits_when_telemetry_cannot_start() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().to_string_lossy().into_owned();
    let args = [
        "--component-root",
        &root,
        "--metrics-addr",
        "127.0.0.1:0",
        "--otlp-endpoint",
        "::bad::",
    ];
    let (code, stderr) = run(&args, None)?;
    assert_eq!(code, Some(1));
    assert!(stderr.contains("OTLP"), "{stderr}");
    Ok(())
}
