use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Map, Value};
use tokio::sync::oneshot;
use trogon_sandbox_contract::StdinEof;

use crate::configuration_item::ConfigurationItem;
use crate::configuration_provider::ConfigurationProvider;
use crate::constants::METHOD_NOT_FOUND;
use crate::encode_message::encode_message;
use crate::lock_ext::LockExt;
use crate::lsp_error::LspError;
use crate::lsp_transport::LspTransport;
use crate::notification_handle::NotificationHandle;
use crate::notification_handler::NotificationHandler;
use crate::notification_registry::NotificationRegistry;
use crate::outgoing_message::{OutgoingNotification, OutgoingRequest, OutgoingResponse, ResponseOutcome};
use crate::request_id::RequestId;

type PendingReply = oneshot::Sender<Result<Value, LspError>>;

/// A JSON-RPC connection to a language server: request and response matching, notification
/// listeners, and the fixed answers to the server's own requests.
pub struct JsonRpcConnection {
    transport: Arc<dyn LspTransport>,
    next_id: AtomicU64,
    pending: Mutex<HashMap<RequestId, PendingReply>>,
    notifications: Arc<NotificationRegistry>,
    request_timeout: Duration,
    configuration: Option<ConfigurationProvider>,
}

impl JsonRpcConnection {
    pub fn new(
        transport: Arc<dyn LspTransport>,
        request_timeout: Duration,
        configuration: Option<ConfigurationProvider>,
    ) -> Arc<Self> {
        Arc::new(Self {
            transport,
            next_id: AtomicU64::new(0),
            pending: Mutex::new(HashMap::new()),
            notifications: Arc::new(NotificationRegistry::default()),
            request_timeout,
            configuration,
        })
    }

    pub async fn request(
        &self,
        method: &str,
        params: Option<Value>,
        timeout: Option<Duration>,
    ) -> Result<Value, LspError> {
        let id = RequestId::new(self.next_id.fetch_add(1, Ordering::Relaxed) + 1);
        let timeout = timeout.unwrap_or(self.request_timeout);
        let (tx, rx) = oneshot::channel();
        self.pending.locked().insert(id, tx);

        let exchange = async {
            self.send(&OutgoingRequest::new(id, method, params)).await?;
            rx.await
                .unwrap_or_else(|_| Err(LspError::NotRunning("connection closed".into())))
        };
        match tokio::time::timeout(timeout, exchange).await {
            Ok(result) => {
                if result.is_err() {
                    self.pending.locked().remove(&id);
                }
                result
            }
            Err(_) => {
                self.pending.locked().remove(&id);
                let mut cancel = Map::new();
                cancel.insert("id".to_owned(), Value::from(id.get()));
                let _ = self.notify("$/cancelRequest", Some(Value::Object(cancel))).await;
                Err(LspError::RequestTimeout {
                    method: method.to_owned(),
                    timeout,
                })
            }
        }
    }

    pub async fn notify(&self, method: &str, params: Option<Value>) -> Result<(), LspError> {
        self.send(&OutgoingNotification::new(method, params)).await
    }

    pub fn on_notification(&self, method: &str, handler: NotificationHandler) -> NotificationHandle {
        let id = self.notifications.add(method, handler);
        NotificationHandle::new(Arc::clone(&self.notifications), method, id)
    }

    /// Feeds one decoded JSON body from the framer.
    pub fn handle_body(&self, body: &str) -> Result<(), LspError> {
        let message: Value =
            serde_json::from_str(body).map_err(|_| LspError::Protocol("LSP message body is not valid JSON".into()))?;
        let Some(object) = message.as_object() else {
            return Err(LspError::Protocol("LSP message body is not an object".into()));
        };
        let Some(method) = object.get("method").and_then(Value::as_str) else {
            self.handle_response(&message);
            return Ok(());
        };
        match object.get("id") {
            Some(id) if !id.is_null() => self.reply_to_server_request(id.clone(), method, object.get("params")),
            _ => self
                .notifications
                .dispatch(method, object.get("params").unwrap_or(&Value::Null)),
        }
        Ok(())
    }

    /// Rejects every in-flight request, e.g. when the server stream ends.
    pub fn close(&self, reason: LspError) {
        let pending: Vec<PendingReply> = self.pending.locked().drain().map(|(_, tx)| tx).collect();
        for tx in pending {
            let _ = tx.send(Err(reason.clone()));
        }
    }

    async fn send<T: serde::Serialize>(&self, message: &T) -> Result<(), LspError> {
        self.transport.write(&encode_message(message)?, StdinEof::Keep).await
    }

    fn handle_response(&self, message: &Value) {
        let Some(id) = message.get("id").and_then(Value::as_u64).map(RequestId::new) else {
            return;
        };
        let Some(tx) = self.pending.locked().remove(&id) else {
            return;
        };
        let outcome = match message.get("error").filter(|e| !e.is_null()) {
            Some(error) => Err(LspError::Response {
                code: error.get("code").and_then(Value::as_i64).unwrap_or(0),
                message: error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                data: error.get("data").cloned(),
            }),
            None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
        };
        let _ = tx.send(outcome);
    }

    fn reply_to_server_request(&self, id: Value, method: &str, params: Option<&Value>) {
        let outcome = match method {
            "workspace/configuration" => ResponseOutcome::Result(Value::Array(self.configuration_answers(params))),
            "client/registerCapability" | "window/workDoneProgress/create" => ResponseOutcome::Result(Value::Null),
            _ => ResponseOutcome::Error {
                code: METHOD_NOT_FOUND,
                message: format!("method not found: {method}"),
            },
        };
        let response = OutgoingResponse::new(id, outcome);
        let transport = Arc::clone(&self.transport);
        tokio::spawn(async move {
            let _ = send_response(transport.as_ref(), &response).await;
        });
    }

    fn configuration_answers(&self, params: Option<&Value>) -> Vec<Value> {
        let items = params
            .and_then(|p| p.get("items"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        items
            .into_iter()
            .map(|item| {
                let item: ConfigurationItem = serde_json::from_value(item).unwrap_or_default();
                self.configuration
                    .as_ref()
                    .map_or(Value::Null, |provide| provide(&item))
            })
            .collect()
    }
}

async fn send_response(transport: &dyn LspTransport, response: &OutgoingResponse) -> Result<(), LspError> {
    transport.write(&encode_message(response)?, StdinEof::Keep).await
}
