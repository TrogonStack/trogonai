use std::sync::{Arc, Mutex, PoisonError};

use futures::{SinkExt, StreamExt};
use kube::Client;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::HeaderValue;

use super::exec_step_tests::ExecStep;

type SharedFrames = Arc<Mutex<Vec<(u8, Vec<u8>)>>>;

const SUBPROTOCOL: &str = "v4.channel.k8s.io";

/// A loopback websocket server that speaks the pod exec channel protocol, so a
/// `kube` client can attach to it exactly as it would to an apiserver.
pub(super) struct FakeExecServer {
    client: Client,
    requests: Arc<Mutex<Vec<String>>>,
    frames: SharedFrames,
}

impl FakeExecServer {
    pub(super) async fn start(script: Vec<ExecStep>) -> Result<Self, Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let requests = Arc::new(Mutex::new(Vec::new()));
        let frames = Arc::new(Mutex::new(Vec::new()));
        tokio::spawn(serve(listener, script, Arc::clone(&requests), Arc::clone(&frames)));
        let config = kube::Config::new(format!("http://{address}").parse()?);
        Ok(Self {
            client: Client::try_from(config)?,
            requests,
            frames,
        })
    }

    pub(super) fn client(&self) -> Client {
        self.client.clone()
    }

    /// The request targets (path and query) the server accepted.
    pub(super) fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// The `(channel, payload)` frames the client sent.
    pub(super) fn frames(&self) -> Vec<(u8, Vec<u8>)> {
        self.frames.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

#[expect(
    clippy::result_large_err,
    reason = "the websocket handshake callback is defined to return an http response as its error"
)]
async fn serve(listener: TcpListener, script: Vec<ExecStep>, requests: Arc<Mutex<Vec<String>>>, frames: SharedFrames) {
    let Ok((stream, _)) = listener.accept().await else {
        return;
    };
    let record = Arc::clone(&requests);
    let accept = tokio_tungstenite::accept_hdr_async(
        stream,
        move |request: &Request, mut response: Response| -> Result<Response, ErrorResponse> {
            record
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(request.uri().to_string());
            response
                .headers_mut()
                .insert("sec-websocket-protocol", HeaderValue::from_static(SUBPROTOCOL));
            Ok(response)
        },
    );
    let Ok(mut socket) = accept.await else {
        return;
    };
    // A script that ends by sending output lets the client finish reading before
    // the connection goes away; closing with the client's frames unread would
    // reset the connection and lose the output already sent.
    let lingers = matches!(script.last(), Some(ExecStep::Out(..)));
    for step in script {
        match step {
            ExecStep::AwaitFrame(channel) => {
                while let Some(Ok(message)) = socket.next().await {
                    let Message::Binary(bytes) = message else {
                        continue;
                    };
                    let Some((first, rest)) = bytes.split_first() else {
                        continue;
                    };
                    frames
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push((*first, rest.to_vec()));
                    if *first == channel && !rest.is_empty() {
                        break;
                    }
                }
            }
            ExecStep::Out(channel, data) => {
                let mut frame = vec![channel];
                frame.extend_from_slice(&data);
                if socket.send(Message::binary(frame)).await.is_err() {
                    return;
                }
            }
        }
    }
    if lingers {
        let drained = async { while socket.next().await.is_some_and(|message| message.is_ok()) {} };
        tokio::time::timeout(std::time::Duration::from_millis(500), drained)
            .await
            .ok();
    }
}
