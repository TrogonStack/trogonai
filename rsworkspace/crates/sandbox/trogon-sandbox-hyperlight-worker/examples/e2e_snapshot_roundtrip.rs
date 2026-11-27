//! Shows that a guest snapshot survives the registry transport without losing state.
//!
//! 1. Loads the module at `--module` into a guest and calls `--guest-function`.
//! 2. Rewinds the guest to its loaded state, then saves a snapshot as an OCI image layout.
//! 3. Packs the layout into a `HyperlightSnapshotV1` artifact and pushes it to `--registry`.
//! 4. Drops the guest, pulls the artifact back and unpacks it.
//! 5. Restores a new guest from the layout, calls the function again and checks the result.
//!
//! It needs a Linux host with /dev/kvm or /dev/mshv, or Windows with WHP, an OCI registry reachable
//! over plain HTTP, and a module the hyperlight-wasm AOT compiler produced. The passthrough guest
//! under `tests/fixtures/passthrough-guest` exports the default `echo_double`:
//!
//! ```text
//! RUSTFLAGS="--cfg trogon_hyperlight_wasm" cargo run -p trogon-sandbox-hyperlight-worker --example e2e_snapshot_roundtrip -- \
//!     --module ./passthrough_guest.aot --registry 127.0.0.1:5000
//! ```

use std::process::ExitCode;

#[cfg(not(all(trogon_hyperlight_wasm, any(target_os = "linux", target_os = "windows"))))]
use std::io::Write;

#[cfg(all(trogon_hyperlight_wasm, any(target_os = "linux", target_os = "windows")))]
use {
    anyhow::Context,
    clap::Parser,
    std::io::Write,
    std::path::PathBuf,
    trogon_sandbox_hyperlight_worker::{
        Hypervisor, WasmGuest, WasmHypervisor, pack_layout_dir, pull_snapshot_blob, push_snapshot_artifact,
        unpack_layout_dir,
    },
    trogon_sandbox_oci::{OciClient, OciClientConfig, RegistryHost},
};

/// Pushes a guest snapshot through a registry and restores it on a new guest.
#[cfg(all(trogon_hyperlight_wasm, any(target_os = "linux", target_os = "windows")))]
#[derive(Parser, Debug)]
#[command(version)]
struct Args {
    /// AOT-compiled guest module to load.
    #[arg(long)]
    module: PathBuf,

    /// Registry to push to and pull from, reached over plain HTTP.
    #[arg(long, default_value = "127.0.0.1:5000")]
    registry: String,

    /// Repository under `--registry` the snapshot artifact goes to.
    #[arg(long, default_value = "snapshots/hyperlight/e2e")]
    repository: String,

    /// Guest function taking and returning an `i32`, called before and after the round trip.
    #[arg(long, default_value = "echo_double")]
    guest_function: String,

    /// Argument passed to `--guest-function`.
    #[arg(long, default_value_t = 21)]
    input: i32,
}

#[cfg(all(trogon_hyperlight_wasm, any(target_os = "linux", target_os = "windows")))]
fn call(guest: &mut WasmGuest, function: &str, input: i32) -> anyhow::Result<i32> {
    guest
        .sandbox_mut()
        .call_guest_function(function, (input,))
        .with_context(|| format!("calling {function}"))
}

#[cfg(all(trogon_hyperlight_wasm, any(target_os = "linux", target_os = "windows")))]
async fn run() -> anyhow::Result<()> {
    let args = Args::parse();
    let mut stdout = std::io::stdout();
    let hypervisor = WasmHypervisor::default();
    anyhow::ensure!(hypervisor.is_available(), "no hypervisor is available on this host");

    let mut guest = hypervisor.load_module(hypervisor.build_warm()?, &args.module)?;
    let before = call(&mut guest, &args.guest_function, args.input)?;
    writeln!(stdout, "{}({}) returned {before}", args.guest_function, args.input)?;

    guest.reset()?;
    let scratch = tempfile::tempdir().context("creating the snapshot directory")?;
    let layout = scratch.path().join("layout");
    hypervisor.save_layout(&mut guest, &layout)?;
    drop(guest);

    let blob = pack_layout_dir(&layout)?;
    let client = OciClient::new(OciClientConfig {
        insecure_registries: vec![RegistryHost::new(args.registry.as_str())?],
        ..Default::default()
    });
    let (artifact, bytes) = push_snapshot_artifact(&client, &args.registry, &args.repository, blob).await?;
    writeln!(stdout, "pushed {bytes} bytes as {}", artifact.digest)?;

    let pulled = pull_snapshot_blob(&client, &artifact).await?;
    let restore = tempfile::tempdir().context("creating the restore directory")?;
    unpack_layout_dir(&pulled, restore.path())?;
    let mut restored = hypervisor.load_layout(hypervisor.build_warm()?, restore.path())?;
    let after = call(&mut restored, &args.guest_function, args.input)?;
    writeln!(
        stdout,
        "{}({}) returned {after} after the restore",
        args.guest_function, args.input
    )?;

    anyhow::ensure!(
        before == after,
        "{} returned {before} before and {after} after the restore",
        args.guest_function
    );
    Ok(())
}

#[cfg(all(trogon_hyperlight_wasm, any(target_os = "linux", target_os = "windows")))]
#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<ExitCode> {
    run().await?;
    Ok(ExitCode::SUCCESS)
}

#[cfg(not(all(trogon_hyperlight_wasm, any(target_os = "linux", target_os = "windows"))))]
fn main() -> ExitCode {
    let _ = writeln!(
        std::io::stderr(),
        "e2e_snapshot_roundtrip needs the trogon_hyperlight_wasm cfg on Linux (KVM or MSHV) or Windows (WHP)"
    );
    ExitCode::from(2)
}
