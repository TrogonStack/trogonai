mod telemetry;

fn main() -> std::process::ExitCode {
    eprintln!("Peer connector runtime is unavailable.");
    std::process::ExitCode::FAILURE
}
