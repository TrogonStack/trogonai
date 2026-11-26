use futures::StreamExt;
use tokio_util::sync::CancellationToken;

use crate::constants::{CONNECT_TIMEOUT, RECONNECT_DELAY};
use crate::jwt_bundle_state::JwtBundleState;
use crate::source_end::SourceEnd;
use crate::source_fault::SourceFault;
use crate::{JwtBundleSet, UnixEndpoint};
use std::sync::Arc;

/// Reads the Workload API `FetchJWTBundles` stream into a
/// [`crate::JwtBundleSource`], reconnecting after any failure. A
/// `PermissionDenied` status withdraws the trusted set at once; any other
/// failure keeps it for the staleness bound.
pub struct JwtBundleDriver {
    state: Arc<JwtBundleState>,
    endpoint: UnixEndpoint,
}

impl JwtBundleDriver {
    pub(crate) fn new(state: Arc<JwtBundleState>, endpoint: UnixEndpoint) -> Self {
        Self { state, endpoint }
    }

    pub async fn run(self, shutdown: CancellationToken) {
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
                    if end.is_some_and(|end| end.fault == SourceFault::PermissionDenied) {
                        self.state.withdraw();
                    }
                    self.state.lost();
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
            crate::workload_stream::connect_jwt_bundles(&self.endpoint),
        )
        .await
        {
            Ok(Ok(stream)) => stream,
            Ok(Err(error)) if error.is_permission_denied() => {
                return Some(SourceEnd::new(SourceFault::PermissionDenied, false));
            }
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
            let response = match update {
                Some(Ok(response)) => response,
                Some(Err(status)) if status.code() == tonic::Code::PermissionDenied => {
                    return Some(SourceEnd::new(SourceFault::PermissionDenied, observed));
                }
                Some(Err(_)) => return Some(SourceEnd::new(SourceFault::StreamError, observed)),
                None => return Some(SourceEnd::new(SourceFault::StreamEnded, observed)),
            };
            match JwtBundleSet::from_wire(response) {
                Ok(bundles) => {
                    observed = true;
                    self.state.replace(bundles);
                }
                Err(error) => return Some(SourceEnd::new(error.into(), observed)),
            }
        }
    }
}

impl Drop for JwtBundleDriver {
    fn drop(&mut self) {
        self.state.lost();
    }
}
