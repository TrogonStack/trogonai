use std::net::SocketAddr;
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::proxy_port_range::ProxyPortRange;
use crate::windows_sandbox_error::WindowsSandboxError;
use crate::windows_sandbox_error_code::WindowsSandboxErrorCode as Code;

const LISTENER_BIND_ATTEMPTS: usize = 5;

/// A bound, non-blocking loopback listener whose port is outside `range`.
fn candidate(range: ProxyPortRange) -> Option<(TcpListener, SocketAddr)> {
    let listener = TcpListener::bind("127.0.0.1:0").ok()?;
    let addr = listener.local_addr().ok()?;
    (!range.contains(addr.port()) && listener.set_nonblocking(true).is_ok()).then_some((listener, addr))
}

pub(crate) struct ProbeListener {
    pub(crate) addr: SocketAddr,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl ProbeListener {
    /// Bind an ephemeral loopback port outside the permit range: a port in
    /// range would be permitted even with the fence active, a false
    /// `connected`. A connect that gets through is accepted and dropped.
    pub(crate) fn bind_outside(range: ProxyPortRange) -> Result<Self, WindowsSandboxError> {
        for _ in 0..LISTENER_BIND_ATTEMPTS {
            let Some((listener, addr)) = candidate(range) else {
                continue;
            };
            let stop = Arc::new(AtomicBool::new(false));
            let flag = Arc::clone(&stop);
            let handle = thread::spawn(move || {
                while !flag.load(Ordering::Relaxed) {
                    if listener.accept().is_err() {
                        thread::sleep(Duration::from_millis(10));
                    }
                }
            });
            return Ok(Self {
                addr,
                stop,
                handle: Some(handle),
            });
        }
        Err(WindowsSandboxError::new(
            Code::WfpVerifyBindFailed,
            format!(
                "could not bind a loopback listener outside the WFP permit range [{},{}] in \
                 {LISTENER_BIND_ATTEMPTS} attempts",
                range.low(),
                range.high()
            ),
        ))
    }
}

impl Drop for ProbeListener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests;
