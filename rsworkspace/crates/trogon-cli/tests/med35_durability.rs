//! MED-35 integration test (requires a real NATS server with JetStream).
//!
//! Verifies that `NatsClient::subscribe_jetstream_bytes` — the JetStream ordered
//! consumer used for prompt notifications — replays messages that were published
//! while the consumer's connection was down, after it reconnects. A toggleable
//! TCP proxy sits between the consumer and NATS so we can sever and restore the
//! consumer's link while the server (and stream) stay up, exactly like a client
//! network blip.
//!
//! Run with a server up:
//!   nats-server -js -p 4222 &
//!   NATS_TEST_URL=nats://localhost:4222 \
//!     cargo test -p trogon-cli --test med35_durability -- --ignored --nocapture

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::task::AbortHandle;
use trogon_cli::NatsClient;

fn nats_url() -> String {
    std::env::var("NATS_TEST_URL").unwrap_or_else(|_| "nats://localhost:4222".to_string())
}

fn host_port(url: &str) -> String {
    url.trim_start_matches("nats://").trim_end_matches('/').to_string()
}

/// A TCP proxy whose forwarding can be cut and restored at runtime.
struct ToggleProxy {
    cut: Arc<AtomicBool>,
    relays: Arc<Mutex<Vec<AbortHandle>>>,
    local_addr: String,
}

impl ToggleProxy {
    async fn start(upstream: String) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap().to_string();
        let cut = Arc::new(AtomicBool::new(false));
        let relays: Arc<Mutex<Vec<AbortHandle>>> = Arc::new(Mutex::new(Vec::new()));
        let (cut2, relays2) = (cut.clone(), relays.clone());
        tokio::spawn(async move {
            loop {
                let Ok((mut client, _)) = listener.accept().await else { break };
                if cut2.load(Ordering::SeqCst) {
                    let _ = client.shutdown().await;
                    continue;
                }
                let upstream = upstream.clone();
                let handle = tokio::spawn(async move {
                    if let Ok(mut up) = TcpStream::connect(&upstream).await {
                        let _ = tokio::io::copy_bidirectional(&mut client, &mut up).await;
                    }
                });
                relays2.lock().unwrap().push(handle.abort_handle());
            }
        });
        Self { cut, relays, local_addr }
    }

    /// Drop all active relays and reject new connections.
    fn cut(&self) {
        self.cut.store(true, Ordering::SeqCst);
        for h in self.relays.lock().unwrap().drain(..) {
            h.abort();
        }
    }

    /// Resume accepting connections.
    fn restore(&self) {
        self.cut.store(false, Ordering::SeqCst);
    }

    fn url(&self) -> String {
        format!("nats://{}", self.local_addr)
    }
}

async fn recv(rx: &mut tokio::sync::mpsc::Receiver<Bytes>, timeout: Duration) -> Option<Bytes> {
    tokio::time::timeout(timeout, rx.recv()).await.ok().flatten()
}

#[tokio::test]
#[ignore = "requires a real NATS server with JetStream (set NATS_TEST_URL)"]
async fn med35_ordered_consumer_replays_after_reconnect() {
    let upstream = nats_url();
    let ns = format!("med35x{}", std::process::id());
    let stream_name = format!("MED35_{}_CLIENT_OPS", std::process::id());
    let subject = format!("{ns}.session.s1.client.session.update");
    let capture = format!("{ns}.session.*.client.>");

    // Publisher connects directly to NATS (its link is never cut).
    let publisher = async_nats::connect(&upstream).await.expect("connect publisher");
    let js = async_nats::jetstream::new(publisher.clone());

    // Provision a stream capturing the notification subject (same shape as the
    // real *_CLIENT_OPS stream). Memory storage is fine: the server stays up
    // across the simulated client blip, so the stream — and its messages — persist.
    let _ = js.delete_stream(&stream_name).await;
    js.create_stream(async_nats::jetstream::stream::Config {
        name: stream_name.clone(),
        subjects: vec![capture.clone()],
        storage: async_nats::jetstream::stream::StorageType::Memory,
        ..Default::default()
    })
    .await
    .expect("create stream");

    // Consumer connects THROUGH the toggle proxy so we can sever its link.
    let proxy = ToggleProxy::start(host_port(&upstream)).await;
    let consumer_client = async_nats::ConnectOptions::new()
        .connection_timeout(Duration::from_secs(5))
        .connect(proxy.url())
        .await
        .expect("connect consumer via proxy");

    // Method under test: JetStream ordered consumer, DeliverPolicy::New.
    let mut rx = consumer_client
        .subscribe_jetstream_bytes(stream_name.clone(), subject.clone())
        .await
        .expect("jetstream subscribe");

    // Let the consumer become ready before the first publish.
    tokio::time::sleep(Duration::from_millis(750)).await;

    // msg1 — delivered while connected.
    publisher.publish(subject.clone(), Bytes::from_static(b"msg1")).await.unwrap();
    publisher.flush().await.unwrap();
    assert_eq!(
        recv(&mut rx, Duration::from_secs(5)).await.as_deref(),
        Some(&b"msg1"[..]),
        "msg1 should arrive while connected"
    );

    // Sever the consumer's link.
    proxy.cut();
    tokio::time::sleep(Duration::from_millis(750)).await;

    // msg2 + msg3 published DURING the outage — core pub-sub would lose these.
    publisher.publish(subject.clone(), Bytes::from_static(b"msg2")).await.unwrap();
    publisher.publish(subject.clone(), Bytes::from_static(b"msg3")).await.unwrap();
    publisher.flush().await.unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Restore connectivity; the ordered consumer should recreate from the last
    // delivered sequence and replay msg2 + msg3 from the retained stream.
    proxy.restore();

    let m2 = recv(&mut rx, Duration::from_secs(20)).await;
    let m3 = recv(&mut rx, Duration::from_secs(20)).await;
    assert_eq!(m2.as_deref(), Some(&b"msg2"[..]), "msg2 must be replayed after reconnect");
    assert_eq!(m3.as_deref(), Some(&b"msg3"[..]), "msg3 must be replayed after reconnect");

    let _ = js.delete_stream(&stream_name).await;
}
