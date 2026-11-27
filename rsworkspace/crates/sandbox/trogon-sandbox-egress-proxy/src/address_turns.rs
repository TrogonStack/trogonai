//! Which vetted address a direct dial tries, in what order and for how long,
//! as a dual-stack connect does it with automatic family selection on: the families
//! alternate starting with the family of the first answer, an attempt that
//! fails hands over at once, an attempt that is silent for 250 ms is closed
//! and handed over, the last attempt has no such limit, and the whole dial
//! ends after 30 seconds.

use std::future::Future;
use std::io;

use tokio::time::timeout;

use crate::resolve::VettedEndpoint;

mod constants;
pub(crate) use constants::{ATTEMPT_DELAY, DIAL_TIMEOUT};

pub(crate) fn in_wire_order(endpoints: &[VettedEndpoint]) -> Vec<VettedEndpoint> {
    let Some(first) = endpoints.first() else {
        return Vec::new();
    };
    let first_is_v4 = first.socket_addr().is_ipv4();
    let (same, other): (Vec<_>, Vec<_>) = endpoints
        .iter()
        .copied()
        .partition(|endpoint| endpoint.socket_addr().is_ipv4() == first_is_v4);
    let mut ordered = Vec::with_capacity(endpoints.len());
    let mut same = same.into_iter();
    let mut other = other.into_iter();
    loop {
        match (same.next(), other.next()) {
            (None, None) => return ordered,
            (a, b) => ordered.extend(a.into_iter().chain(b)),
        }
    }
}

pub(crate) async fn first_to_connect<T, F, Fut>(endpoints: &[VettedEndpoint], mut attempt: F) -> io::Result<T>
where
    F: FnMut(VettedEndpoint) -> Fut,
    Fut: Future<Output = io::Result<T>>,
{
    let ordered = in_wire_order(endpoints);
    let count = ordered.len();
    let dial = async {
        let mut last = None;
        for (index, endpoint) in ordered.into_iter().enumerate() {
            let connecting = attempt(endpoint);
            let outcome = if index + 1 == count {
                connecting.await
            } else {
                timeout(ATTEMPT_DELAY, connecting)
                    .await
                    .unwrap_or_else(|_| Err(io::ErrorKind::TimedOut.into()))
            };
            match outcome {
                Ok(connected) => return Ok(connected),
                Err(err) => last = Some(err),
            }
        }
        Err(last.unwrap_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no addresses")))
    };
    timeout(DIAL_TIMEOUT, dial)
        .await
        .unwrap_or_else(|_| Err(io::Error::new(io::ErrorKind::TimedOut, "connect timed out")))
}

#[cfg(test)]
mod tests;
