use clap::Parser;
use trogon_sandbox_hyperlight_worker::{UnsupportedHypervisor, WorkerArgs, serve};

#[tokio::test]
async fn a_host_without_a_hypervisor_exits_with_failure() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().to_string_lossy().into_owned();
    let args = WorkerArgs::try_parse_from(["worker", "--component-root", &root, "--metrics-addr", "127.0.0.1:0"])?;
    let code = serve(args, UnsupportedHypervisor).await;
    assert_eq!(format!("{code:?}"), format!("{:?}", std::process::ExitCode::FAILURE));
    Ok(())
}
