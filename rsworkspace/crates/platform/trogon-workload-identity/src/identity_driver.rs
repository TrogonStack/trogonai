use futures::StreamExt;
use tokio_util::sync::CancellationToken;

use crate::IdentityDriver;
use crate::candidate::Candidate;
use crate::constants::{CONNECT_TIMEOUT, LIFECYCLE_POLL, RECONNECT_DELAY};
use crate::source_end::SourceEnd;
use crate::source_fault::SourceFault;
use crate::workload_context::WorkloadContext;

impl IdentityDriver {
    pub async fn run(self, shutdown: CancellationToken) {
        tokio::join!(self.read(shutdown.clone()), self.activate(shutdown.clone()), async {
            let mut timer = tokio::time::interval(LIFECYCLE_POLL);
            loop {
                tokio::select! {
                    () = shutdown.cancelled() => break,
                    _ = timer.tick() => self.store.expire(),
                }
            }
        });
    }

    async fn read(&self, shutdown: CancellationToken) {
        let mut previous = None;
        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                end = self.read_connection(shutdown.clone()) => {
                    if let Some(end) = end {
                        let repeated = !end.observed && previous == Some(end.fault);
                        crate::telemetry::source_fault(end.fault, repeated);
                        previous = (!end.observed).then_some(end.fault);
                    }
                    self.store.withdraw_source();
                }
            }
            tokio::select! {
                () = shutdown.cancelled() => break,
                () = tokio::time::sleep(RECONNECT_DELAY) => {},
            }
        }
    }

    /// Reads one connection until it fails. `None` means shutdown ended it.
    async fn read_connection(&self, shutdown: CancellationToken) -> Option<SourceEnd> {
        let mut stream = match tokio::time::timeout(
            CONNECT_TIMEOUT,
            crate::workload_stream::connect(self.store.config().endpoint()),
        )
        .await
        {
            Ok(Ok(stream)) => stream,
            Ok(Err(_)) => return Some(SourceEnd::new(SourceFault::ConnectFailed, false)),
            Err(_) => return Some(SourceEnd::new(SourceFault::ConnectTimedOut, false)),
        };
        let mut observed = false;
        let mut first = true;
        loop {
            let update = if first {
                match tokio::time::timeout(CONNECT_TIMEOUT, stream.next()).await {
                    Ok(update) => update,
                    Err(_) => return Some(SourceEnd::new(SourceFault::FirstResponseTimedOut, observed)),
                }
            } else {
                tokio::select! {
                    () = shutdown.cancelled() => return None,
                    update = stream.next() => update,
                }
            };
            first = false;
            let context = match update {
                Some(Ok(context)) => context,
                Some(Err(_)) => return Some(SourceEnd::new(SourceFault::StreamError, observed)),
                None => return Some(SourceEnd::new(SourceFault::StreamEnded, observed)),
            };
            let candidate = match WorkloadContext::from_wire(context) {
                Ok(context) => {
                    Candidate::parse(context, self.store.config().local().spiffe_id()).map_err(SourceFault::from)
                }
                Err(error) => Err(SourceFault::from(&error)),
            };
            match candidate {
                Ok(candidate) => {
                    observed = true;
                    self.store.observe(candidate);
                }
                Err(fault) => return Some(SourceEnd::new(fault, observed)),
            }
        }
    }

    async fn activate(&self, shutdown: CancellationToken) {
        let mut changes = self.store.inner.events.subscribe();
        loop {
            changes.borrow_and_update();
            if let Some(pending) = self.store.prepare() {
                tokio::select! {
                    () = shutdown.cancelled() => break,
                    result = changes.changed() => { if result.is_err() { break; } continue; },
                    result = tokio::time::timeout(CONNECT_TIMEOUT, self.store.inner.audit.record(pending.fact())) => {
                        if matches!(result, Ok(Ok(()))) { self.store.activate(pending); }
                    }
                }
            }
            tokio::select! {
                () = shutdown.cancelled() => break,
                result = changes.changed() => { if result.is_err() { break; } },
                () = tokio::time::sleep(RECONNECT_DELAY) => {},
            }
        }
    }
}

impl Drop for IdentityDriver {
    fn drop(&mut self) {
        self.store.withdraw_source();
    }
}
