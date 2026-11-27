use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use trogon_sandbox_contract::UnixSignal;

pub(crate) fn to_signal(signal: UnixSignal) -> Signal {
    match signal {
        UnixSignal::Term => Signal::SIGTERM,
        UnixSignal::Kill => Signal::SIGKILL,
        UnixSignal::Int => Signal::SIGINT,
        UnixSignal::Hup => Signal::SIGHUP,
        UnixSignal::Quit => Signal::SIGQUIT,
        UnixSignal::Usr1 => Signal::SIGUSR1,
        UnixSignal::Usr2 => Signal::SIGUSR2,
    }
}

/// The process group a child leads, taken while the child is still known to be running.
pub(crate) fn group_of(child_id: Option<u32>) -> Option<Pid> {
    child_id.and_then(|id| i32::try_from(id).ok()).map(Pid::from_raw)
}

/// Signals the whole group, so a shell's own children receive it too.
pub(crate) fn deliver_signal(group: Option<Pid>, signal: UnixSignal) {
    group.inspect(|group| {
        // The group may already be gone, which is exactly the outcome a kill wants.
        let _ = killpg(*group, to_signal(signal));
    });
}

#[cfg(test)]
mod tests;
