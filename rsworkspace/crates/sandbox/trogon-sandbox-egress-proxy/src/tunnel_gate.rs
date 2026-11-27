//! The limits on terminated tunnels that several commands share and that can
//! change while they run: how many may be open, how long a handshake may
//! take, and who has been told the cap is reached.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use trogon_sandbox_contract::{NetworkOptions, TunnelCap};
use trogon_sandbox_protocol::DEFAULT_TUNNEL_CAP;

mod constants;
use constants::{MAX_AUDIENCES, UNSET};

/// The cap and handshake timeout are read at each CONNECT, so a change reaches
/// the tunnels that start after it. The count of terminated tunnels is shared
/// by everything holding the gate.
#[derive(Debug, Default)]
pub struct TunnelGate {
    cap: AtomicUsize,
    handshake_ms: AtomicU64,
    terminated: AtomicUsize,
    audiences: AtomicU64,
    told: Mutex<HashSet<u64>>,
}

/// One terminated tunnel's place under the cap, given back when dropped.
#[derive(Debug)]
pub struct TunnelSlot {
    gate: Arc<TunnelGate>,
}

impl TunnelGate {
    pub fn new(cap: Option<TunnelCap>, handshake: Option<Duration>) -> Self {
        let gate = Self::default();
        gate.set_cap(cap);
        gate.set_handshake(handshake);
        gate
    }

    pub fn from_options(options: &NetworkOptions) -> Self {
        Self::new(options.tunnel_cap, options.handshake_timeout)
    }

    pub fn set_cap(&self, cap: Option<TunnelCap>) {
        self.cap
            .store(cap.map_or(UNSET, |cap| cap.get() as usize), Ordering::Relaxed);
    }

    pub fn set_handshake(&self, timeout: Option<Duration>) {
        let millis = timeout.map_or(0, |timeout| timeout.as_millis().max(1) as u64);
        self.handshake_ms.store(millis, Ordering::Relaxed);
    }

    pub fn cap(&self) -> Option<TunnelCap> {
        u32::try_from(self.cap.load(Ordering::Relaxed))
            .ok()
            .and_then(|cap| TunnelCap::new(cap).ok())
    }

    pub fn handshake(&self) -> Option<Duration> {
        match self.handshake_ms.load(Ordering::Relaxed) {
            0 => None,
            millis => Some(Duration::from_millis(millis)),
        }
    }

    /// The cap in force: the configured one, or the default of 256.
    pub fn limit(&self) -> u32 {
        self.cap().map_or(DEFAULT_TUNNEL_CAP, TunnelCap::get)
    }

    pub fn terminated(&self) -> usize {
        self.terminated.load(Ordering::Relaxed)
    }

    /// A slot for a tunnel about to be terminated, or the cap that is full.
    pub fn take(self: &Arc<Self>) -> Result<TunnelSlot, u32> {
        let cap = self.limit();
        self.terminated
            .try_update(Ordering::AcqRel, Ordering::Acquire, |held| {
                (held < cap as usize).then_some(held + 1)
            })
            .map(|_| TunnelSlot { gate: Arc::clone(self) })
            .map_err(|_| cap)
    }

    /// A name for one command's share of the "cap reached" notices.
    pub fn audience(&self) -> u64 {
        self.audiences.fetch_add(1, Ordering::Relaxed)
    }

    /// Whether `audience` has not been told since the tunnels in use last fell
    /// to half the cap; asking marks it told.
    pub fn first_refusal(&self, audience: u64) -> bool {
        let mut told = self.told.lock().unwrap_or_else(|p| p.into_inner());
        if told.contains(&audience) {
            return false;
        }
        if told.len() >= MAX_AUDIENCES {
            told.clear();
        }
        told.insert(audience);
        true
    }
}

impl Drop for TunnelSlot {
    fn drop(&mut self) {
        let left = self.gate.terminated.fetch_sub(1, Ordering::AcqRel) - 1;
        if left * 2 <= self.gate.limit() as usize {
            self.gate.told.lock().unwrap_or_else(|p| p.into_inner()).clear();
        }
    }
}

#[cfg(test)]
mod tests;
