use std::io;

use tokio::process::Command;

/// Asks the kernel to send the child SIGTERM when this process dies, so a
/// SIGKILLed supervisor does not leave its stack running. `kill_on_drop` only
/// covers an orderly exit.
///
/// The signal fires when the spawning thread exits, not the process, so every
/// spawn goes through the spawner thread, which lives as long as the process.
pub(crate) fn arm(command: &mut Command) {
    let parent = std::process::id();
    // SAFETY: `arm_in_child` only calls async-signal-safe libc functions and allocates nothing between fork and exec.
    unsafe {
        command.pre_exec(move || arm_in_child(parent));
    }
}

/// Runs in the forked child before exec.
fn arm_in_child(parent: u32) -> io::Result<()> {
    // SAFETY: prctl with these arguments only sets a flag on the calling process.
    let status = unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) };
    (status != -1).then_some(()).ok_or_else(io::Error::last_os_error)?;
    // The supervisor may have died before prctl ran; the kernel then
    // never sends the signal, so stop here instead.
    // SAFETY: getppid and raise take no pointers.
    if unsafe { libc::getppid() } as u32 != parent {
        unsafe { libc::raise(libc::SIGTERM) };
    }
    Ok(())
}

#[cfg(test)]
mod tests;
