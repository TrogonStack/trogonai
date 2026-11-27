//! The connections plain-HTTP requests reach a parent proxy over: one pool
//! for every client connection, keyed by the parent's address, that hands out
//! the connection freed last, keeps at most 256 free ones, and closes one
//! left free for 5 s, or for the response's `Keep-Alive: timeout=` less 1 s
//! when that is shorter, and keeps none when that leaves nothing.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Waker};
use std::time::Duration;

use tokio::io::{AsyncRead, ReadBuf};
use trogon_sandbox_contract::{ParentProxyUrl, ProxyScheme};

use crate::upstream_stream::UpstreamStream;

mod constants;
use constants::{FREE_TIMEOUT, HINT_BUFFER, MAX_FREE};

/// Which pool a parent's connections share: a pooling HTTP client agent names a socket by
/// host and port, and http and https parents are on agents of their own.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PoolKey {
    secured: bool,
    host: String,
    port: u16,
}

impl From<&ParentProxyUrl> for PoolKey {
    fn from(proxy: &ParentProxyUrl) -> Self {
        Self {
            secured: proxy.scheme() == ProxyScheme::Https,
            host: proxy.host().to_string(),
            port: proxy.port().get(),
        }
    }
}

/// What a response's `Keep-Alive` field asks of the connection it came on,
/// read as a pooling HTTP client agent reads it: `^timeout=(\d+)` on the field's value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct KeepAliveHint(Option<u64>);

impl KeepAliveHint {
    pub(crate) fn none() -> Self {
        Self(None)
    }

    pub(crate) fn parse(value: &str) -> Self {
        let digits: String = value
            .strip_prefix("timeout=")
            .unwrap_or_default()
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        Self(match digits.is_empty() {
            true => None,
            false => Some(digits.parse().unwrap_or(u64::MAX)),
        })
    }

    /// How long a freed connection is kept: `None` when it is not kept.
    fn free_for(self) -> Option<Duration> {
        let Some(seconds) = self.0 else {
            return Some(FREE_TIMEOUT);
        };
        let hinted = Duration::from_secs(seconds).saturating_sub(HINT_BUFFER);
        if hinted.is_zero() {
            None
        } else {
            Some(hinted.min(FREE_TIMEOUT))
        }
    }
}

struct Parked {
    id: u64,
    stream: UpstreamStream,
}

#[derive(Default)]
struct Pools {
    next_id: u64,
    free: HashMap<PoolKey, Vec<Parked>>,
}

#[derive(Clone, Default)]
pub struct ParentPool {
    pools: Arc<Mutex<Pools>>,
}

impl ParentPool {
    /// The connection to `key` freed last that is still open.
    pub(crate) fn take(&self, key: &PoolKey) -> Option<UpstreamStream> {
        let mut pools = self.lock();
        let free = pools.free.get_mut(key)?;
        while let Some(mut parked) = free.pop() {
            if !has_closed(&mut parked.stream) {
                return Some(parked.stream);
            }
        }
        None
    }

    /// Frees `stream` after a response that left it able to carry another
    /// request, keeping it if the pool has room and `hint` allows.
    pub(crate) fn give_back(&self, key: PoolKey, stream: UpstreamStream, hint: KeepAliveHint) {
        let Some(free_for) = hint.free_for() else {
            return;
        };
        let id = {
            let mut pools = self.lock();
            pools.next_id += 1;
            let id = pools.next_id;
            let free = pools.free.entry(key.clone()).or_default();
            if free.len() >= MAX_FREE {
                return;
            }
            free.push(Parked { id, stream });
            id
        };
        let pools = Arc::downgrade(&self.pools);
        tokio::spawn(async move {
            tokio::time::sleep(free_for).await;
            expire(&pools, &key, id);
        });
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Pools> {
        self.pools.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn expire(pools: &Weak<Mutex<Pools>>, key: &PoolKey, id: u64) {
    let Some(pools) = pools.upgrade() else {
        return;
    };
    let mut pools = pools.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(free) = pools.free.get_mut(key) {
        free.retain(|parked| parked.id != id);
        if free.is_empty() {
            pools.free.remove(key);
        }
    }
}

/// A free connection has nothing to say, so anything readable on it now (an
/// end of stream, a reset or stray bytes) means the parent is done with it.
fn has_closed(stream: &mut UpstreamStream) -> bool {
    let mut probe = [0u8; 1];
    let mut buf = ReadBuf::new(&mut probe);
    let mut cx = Context::from_waker(Waker::noop());
    Pin::new(stream).poll_read(&mut cx, &mut buf).is_ready()
}

#[cfg(test)]
mod tests;
