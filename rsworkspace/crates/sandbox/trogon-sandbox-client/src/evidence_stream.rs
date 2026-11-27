use std::pin::Pin;
use std::task::{Context, Poll};

use futures::Stream;
use futures::stream::BoxStream;
use trogon_sandbox_contract::LeaseId;
use trogon_sandbox_protocol::subjects;

use crate::{BusMessage, ClientError, EvidenceFrame, SandboxClient};

/// The evidence frames published for one lease, in arrival order.
pub struct EvidenceStream {
    lease_id: LeaseId,
    inner: BoxStream<'static, BusMessage>,
}

impl Stream for EvidenceStream {
    type Item = Result<EvidenceFrame, ClientError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.inner).poll_next(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Ready(Some(message)) => {
                let subject_suffix = subjects::evidence_suffix(&self.lease_id, &message.subject)
                    .unwrap_or_else(|| message.subject.clone());
                Poll::Ready(Some(Ok(EvidenceFrame {
                    subject_suffix,
                    payload: message.payload,
                })))
            }
        }
    }
}

impl SandboxClient {
    /// Subscribes to every evidence frame of `lease_id`.
    pub async fn evidence_stream(&self, lease_id: &LeaseId) -> Result<EvidenceStream, ClientError> {
        let inner = self.bus.subscribe(&subjects::evidence_subject(lease_id)).await?;
        Ok(EvidenceStream {
            lease_id: lease_id.clone(),
            inner,
        })
    }
}
