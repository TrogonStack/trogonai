use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

static SIGNALED: AtomicBool = AtomicBool::new(false);

extern "C" fn remember(_: i32) {
    SIGNALED.store(true, Ordering::Relaxed);
}

#[expect(
    clippy::unwrap_used,
    reason = "Native fixture failures must terminate the test immediately"
)]
pub(super) fn run(directory: &Path) -> i32 {
    unsafe { libc::signal(libc::SIGUSR1, remember as *const () as libc::sighandler_t) };
    let child_directory = directory.to_path_buf();
    let worker = std::thread::spawn(move || {
        let mut child = Command::new("/bin/sh")
            .args([
                "-c",
                "directory=$1; trap 'printf X >> \"$directory/child-signals\"' USR1; printf '%s\\n' \"$$\" > \"$directory/child-pid\"; while [ ! -f \"$directory/finish\" ]; do :; done; exit 7",
                "sh",
            ])
            .arg(child_directory)
            .spawn()
            .unwrap();
        child.wait().unwrap().code()
    });
    let until = Instant::now() + Duration::from_secs(15);
    while !directory.join("finish").exists() && Instant::now() < until {
        if SIGNALED.swap(false, Ordering::Relaxed) {
            std::fs::write(directory.join("root-signals"), b"X").unwrap();
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    if worker.join().unwrap() == Some(7) { 7 } else { 90 }
}
