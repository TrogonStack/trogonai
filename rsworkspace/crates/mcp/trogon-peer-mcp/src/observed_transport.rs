use rmcp::service::{RoleServer, RxJsonRpcMessage, TxJsonRpcMessage};
use rmcp::transport::Transport;
use tokio_util::sync::CancellationToken;

pub(crate) struct ObservedTransport<T> {
    inner: T,
    closed: CancellationToken,
}
impl<T> ObservedTransport<T> {
    pub(crate) fn new(inner: T, closed: CancellationToken) -> Self {
        Self { inner, closed }
    }
}
impl<T: Transport<RoleServer>> Transport<RoleServer> for ObservedTransport<T> {
    type Error = T::Error;
    fn send(
        &mut self,
        message: TxJsonRpcMessage<RoleServer>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        self.inner.send(message)
    }
    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleServer>> {
        let message = self.inner.receive().await;
        if message.is_none() {
            self.closed.cancel();
        }
        message
    }
    fn close(&mut self) -> impl Future<Output = Result<(), Self::Error>> + Send {
        self.inner.close()
    }
}
