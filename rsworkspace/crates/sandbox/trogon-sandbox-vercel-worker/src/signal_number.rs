use trogon_sandbox_contract::UnixSignal;

/// The signal number Vercel's kill endpoint takes.
pub(crate) fn signal_number(signal: UnixSignal) -> u32 {
    match signal {
        UnixSignal::Hup => 1,
        UnixSignal::Int => 2,
        UnixSignal::Quit => 3,
        UnixSignal::Kill => 9,
        UnixSignal::Usr1 => 10,
        UnixSignal::Usr2 => 12,
        UnixSignal::Term => 15,
    }
}

#[cfg(test)]
mod tests;
