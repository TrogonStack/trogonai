#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "linux")]
use trogon_sandbox_firecracker_worker::relay;

#[cfg(target_os = "linux")]
fn main() -> anyhow::Result<()> {
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == relay::RELAY_SUBCOMMAND)
    {
        std::process::exit(relay::run(std::env::args_os().skip(2)));
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(linux::main())
}

#[cfg(not(target_os = "linux"))]
fn main() {
    let _ = std::io::Write::write_all(
        &mut std::io::stderr(),
        b"trogon-sandbox-firecracker-worker needs Linux with /dev/kvm; Firecracker does not run elsewhere\n",
    );
    std::process::exit(1);
}

#[cfg(not(target_os = "linux"))]
use trogon_sandbox_observability as _;
