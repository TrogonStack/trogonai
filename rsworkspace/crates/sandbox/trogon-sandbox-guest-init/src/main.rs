#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "linux")]
fn main() {
    if let Err(error) = linux::run() {
        eprintln!("guest init: {error}");
        if std::process::id() == 1 {
            loop {
                std::thread::park();
            }
        }
        std::process::exit(1);
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("trogon-sandbox-guest-init requires Linux");
    std::process::exit(1);
}
