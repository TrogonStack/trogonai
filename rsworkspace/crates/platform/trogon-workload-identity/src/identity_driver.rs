use futures::StreamExt;
use tokio_util::sync::CancellationToken;

use crate::IdentityDriver;
use crate::candidate::Candidate;
use crate::constants::{CONNECT_TIMEOUT, LIFECYCLE_POLL, RECONNECT_DELAY};

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
        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                () = self.read_connection(shutdown.clone()) => self.store.withdraw_source(),
            }
            tokio::select! {
                () = shutdown.cancelled() => break,
                () = tokio::time::sleep(RECONNECT_DELAY) => {},
            }
        }
    }

    async fn read_connection(&self, shutdown: CancellationToken) {
        let Ok(Ok(mut stream)) = tokio::time::timeout(
            CONNECT_TIMEOUT,
            crate::workload_stream::connect(self.store.config().endpoint()),
        )
        .await
        else {
            return;
        };
        let first = tokio::time::timeout(CONNECT_TIMEOUT, stream.next()).await;
        let Ok(Some(Ok(context))) = first else {
            return;
        };
        let Ok(context) = crate::workload_context::WorkloadContext::from_wire(context) else {
            return;
        };
        let Ok(candidate) = Candidate::parse(context, self.store.config().local().spiffe_id()) else {
            return;
        };
        self.store.observe(candidate);
        loop {
            let context = tokio::select! {
                () = shutdown.cancelled() => return,
                update = stream.next() => update,
            };
            let Some(Ok(context)) = context else {
                return;
            };
            let Ok(context) = crate::workload_context::WorkloadContext::from_wire(context) else {
                return;
            };
            let Ok(candidate) = Candidate::parse(context, self.store.config().local().spiffe_id()) else {
                return;
            };
            self.store.observe(candidate);
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
