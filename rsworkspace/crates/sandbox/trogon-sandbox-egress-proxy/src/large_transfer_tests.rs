//! Large bodies through terminated tunnels, over real TLS on both legs:
//! responses to slow readers share the byte budget and arrive whole, and an
//! upload to an upstream that stops reading waits in the client rather than in
//! the proxy, then arrives whole.

use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, mpsc, watch};
use tokio::time::{sleep, timeout};
use tokio_rustls::TlsAcceptor;

use crate::bulk_transfers::{self, bounded};
use crate::dial::UpstreamMarking;
use crate::substitution_tests::TestResult;
use crate::tls_ca::{PermittedDomains, SandboxCa, install_default_crypto_provider};
use crate::tls_intercept::tests::patient_deadlines;
use crate::tls_intercept::tests::{build_test_upstream_ca_and_leaf, client_trusting, root_store_from_pem, vetted};
use crate::tls_intercept::{PrefixedStream, terminate_and_forward_checked};
use crate::tunnel_deadlines::{ByteBudget, SharedQueueBudget, TunnelDeadlines};

const HOST: &str = "api.example.com";
const MIB: usize = 1 << 20;
const CHUNK: usize = 64 * 1024;

type ClientTls = tokio_rustls::client::TlsStream<TcpStream>;
type UpstreamTls = tokio_rustls::server::TlsStream<TcpStream>;

/// The chunk every body here is made of, so that its digest is known.
fn chunk(seed: usize) -> Vec<u8> {
    (0..CHUNK).map(|i| ((i * seed + (i >> 8)) & 0xff) as u8).collect()
}

fn digest_of(chunk: &[u8], size: usize) -> String {
    let mut whole = Sha256::new();
    for _ in 0..size / chunk.len() {
        whole.update(chunk);
    }
    hex::encode(whole.finalize())
}

/// A client stream that counts what the proxy has read off it.
struct CountedReads {
    inner: TcpStream,
    read: Arc<AtomicU64>,
}

impl AsyncRead for CountedReads {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        let polled = Pin::new(&mut self.inner).poll_read(cx, buf);
        let got = buf.filled().len() - before;
        self.read.fetch_add(got as u64, Ordering::SeqCst);
        polled
    }
}

impl AsyncWrite for CountedReads {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// A TLS upstream for [`HOST`] that serves each connection with `serve`.
async fn upstream<F, Fut>(serve: F) -> TestResult<(std::net::SocketAddr, String)>
where
    F: Fn(UpstreamTls) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    install_default_crypto_provider();
    let ca = build_test_upstream_ca_and_leaf(HOST)?;
    let acceptor = TlsAcceptor::from(Arc::new(
        rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![ca.cert.clone()], ca.key.clone_key())?,
    ));
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let serve = Arc::new(serve);
    tokio::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            let serve = Arc::clone(&serve);
            tokio::spawn(async move {
                if let Ok(tls) = acceptor.accept(tcp).await {
                    serve(tls).await;
                }
            });
        }
    });
    Ok((addr, ca.pem))
}

async fn read_head<S: AsyncRead + Unpin>(stream: &mut S) -> Option<String> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte).await {
            Ok(1..) => head.push(byte[0]),
            _ => return None,
        }
    }
    Some(String::from_utf8_lossy(&head).into_owned())
}

fn content_length(head: &str) -> TestResult<usize> {
    let lowered = head.to_ascii_lowercase();
    match lowered.lines().find_map(|line| line.strip_prefix("content-length:")) {
        Some(value) => Ok(value.trim().parse()?),
        None => Ok(0),
    }
}

/// A proxy terminating every tunnel to `upstream`, all of them sharing
/// `budget`; `read` counts what it has read off its clients.
struct Proxy {
    addr: std::net::SocketAddr,
    connector: tokio_rustls::TlsConnector,
    authority: String,
    read: Arc<AtomicU64>,
}

impl Proxy {
    async fn start(upstream: (std::net::SocketAddr, String), budget: SharedQueueBudget) -> TestResult<Self> {
        let (upstream_addr, upstream_pem) = upstream;
        let upstream_tls = Arc::new(
            rustls::ClientConfig::builder()
                .with_root_certificates(root_store_from_pem(&upstream_pem)?)
                .with_no_client_auth(),
        );
        let sandbox_ca = Arc::new(SandboxCa::generate(PermittedDomains::for_tests(&["*.example.com"]))?);
        let connector = client_trusting(&sandbox_ca)?;
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let read = Arc::new(AtomicU64::new(0));
        let counted = Arc::clone(&read);
        tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                let client = CountedReads {
                    inner: tcp,
                    read: Arc::clone(&counted),
                };
                let sandbox_ca = Arc::clone(&sandbox_ca);
                let upstream_tls = Arc::clone(&upstream_tls);
                let budget = budget.clone();
                tokio::spawn(async move {
                    let _ = terminate_and_forward_checked(
                        PrefixedStream::new(client, vec![]),
                        HOST,
                        &sandbox_ca,
                        vetted(upstream_addr).into(),
                        UpstreamMarking::NotNeeded,
                        upstream_tls,
                        None,
                        None,
                        None,
                        TunnelDeadlines {
                            idle: Duration::from_secs(300),
                            stalled: Duration::from_secs(300),
                            ..patient_deadlines()
                        },
                        None,
                        None,
                        Some(budget),
                        None,
                    )
                    .await;
                });
            }
        });
        Ok(Self {
            addr,
            connector,
            authority: format!("{HOST}:{}", upstream_addr.port()),
            read,
        })
    }

    async fn tunnel(&self) -> TestResult<ClientTls> {
        let tcp = bounded("connecting to the proxy", TcpStream::connect(self.addr)).await??;
        Ok(bounded(
            "the client handshake",
            self.connector
                .connect(rustls::pki_types::ServerName::try_from(HOST)?, tcp),
        )
        .await??)
    }
}

/// What a client read of a body: how many bytes, and their SHA-256.
#[derive(Debug, PartialEq, Eq)]
struct Received {
    bytes: usize,
    digest: String,
}

/// One GET in a tunnel of its own, its body left unread until `start` is
/// raised, then taken a chunk at a time with a yield between. `start` is a
/// latch: a client that reaches it after it was raised still goes on, which a
/// `Notify::notify_waiters` wakeup would not guarantee.
async fn get(proxy: &Proxy, path: &str, mut start: watch::Receiver<bool>) -> TestResult<Received> {
    let mut tls = proxy.tunnel().await?;
    bounded(
        "writing the GET",
        tls.write_all(
            format!(
                "GET {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
                proxy.authority
            )
            .as_bytes(),
        ),
    )
    .await??;
    let head = bounded("the GET's head", read_head(&mut tls))
        .await?
        .ok_or("no answer")?;
    assert!(head.starts_with("HTTP/1.1 200 "), "{head}");
    bounded("the slow readers' start", start.wait_for(|raised| *raised)).await??;
    let length = content_length(&head)?;
    let mut hash = Sha256::new();
    let mut bytes = 0;
    let mut buf = vec![0u8; CHUNK];
    while bytes < length {
        let Ok(Ok(n @ 1..)) = bounded("reading the body", tls.read(&mut buf)).await else {
            break;
        };
        bytes += n;
        hash.update(&buf[..n]);
        tokio::task::yield_now().await;
    }
    Ok(Received {
        bytes,
        digest: hex::encode(hash.finalize()),
    })
}

async fn until_used_is_zero(budget: &SharedQueueBudget) -> ByteBudget {
    for _ in 0..100 {
        if budget.used() == ByteBudget::from_bytes(0) {
            break;
        }
        sleep(Duration::from_millis(10)).await;
    }
    budget.used()
}

#[tokio::test]
async fn two_large_responses_to_slow_readers_share_a_small_budget_arrive_whole_and_leave_it_empty()
-> Result<(), Box<dyn std::error::Error>> {
    let _bulk = bulk_transfers::bulk().await?;
    const SIZE: usize = 48 * MIB;
    let body = Arc::new(chunk(31));
    let whole = digest_of(&body, SIZE);
    let (backed_up, mut backups) = mpsc::unbounded_channel::<()>();
    let served = Arc::clone(&body);
    let up = upstream(move |mut tls| {
        let body = Arc::clone(&served);
        let backed_up = backed_up.clone();
        async move {
            let Some(head) = read_head(&mut tls).await else {
                return;
            };
            if !head.starts_with("GET /large ") {
                let _ = tls
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nsmall")
                    .await;
                let _ = tls.shutdown().await;
                return;
            }
            let answer = format!("HTTP/1.1 200 OK\r\nContent-Length: {SIZE}\r\n\r\n");
            if tls.write_all(answer.as_bytes()).await.is_err() {
                return;
            }
            let mut reported = false;
            for _ in 0..SIZE / CHUNK {
                let write = tls.write_all(&body);
                tokio::pin!(write);
                if !reported && timeout(Duration::from_millis(50), &mut write).await.is_err() {
                    reported = true;
                    let _ = backed_up.send(());
                }
                if write.await.is_err() {
                    return;
                }
            }
            let _ = tls.flush().await;
            let _ = tls.shutdown().await;
        }
    })
    .await?;
    let budget = SharedQueueBudget::new(ByteBudget::from_bytes(30_000));
    let proxy = Proxy::start(up, budget.clone()).await?;

    let (start, started) = watch::channel(false);
    let both_backed_up = async move {
        bounded("the first backed-up response", backups.recv()).await?;
        bounded("the second backed-up response", backups.recv()).await?;
        start.send_replace(true);
        TestResult::Ok(())
    };
    let (a, b, backed_up) = tokio::join!(
        get(&proxy, "/large", started.clone()),
        get(&proxy, "/large", started),
        both_backed_up,
    );
    backed_up?;
    let (a, b) = (a?, b?);
    let expected = Received {
        bytes: SIZE,
        digest: whole,
    };
    assert_eq!(a, expected);
    assert_eq!(b, expected);
    assert_eq!(until_used_is_zero(&budget).await, ByteBudget::from_bytes(0));

    let (_ready, started) = watch::channel(true);
    let after = get(&proxy, "/small", started).await?;
    assert_eq!(after.bytes, 5);
    assert_eq!(until_used_is_zero(&budget).await, ByteBudget::from_bytes(0));
    Ok(())
}

#[tokio::test]
async fn an_upload_to_a_stalled_upstream_is_held_back_in_the_client_then_arrives_whole()
-> Result<(), Box<dyn std::error::Error>> {
    let _bulk = bulk_transfers::bulk().await?;
    const SIZE: usize = 96 * MIB;
    let release = Arc::new(Notify::new());
    let released = Arc::clone(&release);
    let up = upstream(move |mut tls| {
        let released = Arc::clone(&released);
        async move {
            let Some(head) = read_head(&mut tls).await else {
                return;
            };
            let Ok(size) = content_length(&head) else {
                return;
            };
            released.notified().await;
            let mut hash = Sha256::new();
            let mut got = 0;
            let mut buf = vec![0u8; CHUNK];
            while got < size {
                let Ok(n @ 1..) = tls.read(&mut buf).await else {
                    return;
                };
                got += n;
                hash.update(&buf[..n]);
            }
            let answer = format!("{got} {}", hex::encode(hash.finalize()));
            let _ = tls
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}",
                        answer.len()
                    )
                    .as_bytes(),
                )
                .await;
            let _ = tls.shutdown().await;
        }
    })
    .await?;
    let budget = SharedQueueBudget::new(ByteBudget::from_bytes(256 * MIB));
    let proxy = Proxy::start(up, budget.clone()).await?;

    let tls = proxy.tunnel().await?;
    let (mut reader, mut writer) = tokio::io::split(tls);
    let body = chunk(7);
    let whole = digest_of(&body, SIZE);
    let head = format!(
        "POST /upload HTTP/1.1\r\nHost: {}\r\nContent-Length: {SIZE}\r\n\r\n",
        proxy.authority
    );
    let upload = tokio::spawn(async move {
        writer.write_all(head.as_bytes()).await?;
        for _ in 0..SIZE / CHUNK {
            writer.write_all(&body).await?;
        }
        writer.flush().await?;
        Ok::<_, io::Error>(writer)
    });

    let mut read = u64::MAX;
    let mut stable = 0;
    for _ in 0..50 {
        if stable >= 4 {
            break;
        }
        sleep(Duration::from_millis(250)).await;
        let now = proxy.read.load(Ordering::SeqCst);
        stable = if now == read { stable + 1 } else { 0 };
        read = now;
    }
    assert!(read < 32 * MIB as u64, "the proxy read {read} bytes of the upload");

    release.notify_one();
    // Moving 96 MiB over two TLS legs takes as long as the runner makes it
    // take, so the answer gets a deadline far past that: an upload that
    // stalls for good fails here instead of holding the bulk lock forever.
    let head = bounded("the answer to the upload", read_head(&mut reader))
        .await?
        .ok_or("expected a value")?;
    let mut body = vec![0u8; content_length(&head)?];
    bounded("the upload's answer body", reader.read_exact(&mut body)).await??;
    let body = String::from_utf8(body)?;
    let _writer = bounded("the upload to finish", upload).await???;
    assert!(head.starts_with("HTTP/1.1 200 "), "{head}");
    assert_eq!(body, format!("{SIZE} {whole}"));
    assert_eq!(until_used_is_zero(&budget).await, ByteBudget::from_bytes(0));
    Ok(())
}

/// Throughput of a large response relayed through a terminated tunnel, to
/// compare relay changes by hand:
/// `cargo test --release -p trogon-sandbox-egress-proxy relayed_response_throughput -- --ignored --nocapture`
#[tokio::test]
#[ignore = "a measurement, not a check"]
async fn relayed_response_throughput() -> Result<(), Box<dyn std::error::Error>> {
    let _bulk = bulk_transfers::bulk().await?;
    const SIZE: usize = 256 * MIB;
    let body = Arc::new(chunk(31));
    let up = upstream(move |mut tls| {
        let body = Arc::clone(&body);
        async move {
            if read_head(&mut tls).await.is_none() {
                return;
            }
            let answer = format!("HTTP/1.1 200 OK\r\nContent-Length: {SIZE}\r\n\r\n");
            if tls.write_all(answer.as_bytes()).await.is_err() {
                return;
            }
            for _ in 0..SIZE / CHUNK {
                if tls.write_all(&body).await.is_err() {
                    return;
                }
            }
            let _ = tls.flush().await;
            let _ = tls.shutdown().await;
        }
    })
    .await?;
    let budget = SharedQueueBudget::new(ByteBudget::from_bytes(64 * MIB));
    let proxy = Proxy::start(up, budget).await?;
    let mut tls = proxy.tunnel().await?;
    tls.write_all(
        format!(
            "GET /large HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
            proxy.authority
        )
        .as_bytes(),
    )
    .await?;
    let head = read_head(&mut tls).await.ok_or("expected a value")?;
    assert!(head.starts_with("HTTP/1.1 200 "), "{head}");
    let started = std::time::Instant::now();
    let mut bytes = 0;
    let mut buf = vec![0u8; CHUNK];
    while bytes < SIZE {
        let Ok(n @ 1..) = tls.read(&mut buf).await else {
            break;
        };
        bytes += n;
    }
    assert_eq!(bytes, SIZE);
    let secs = started.elapsed().as_secs_f64();
    println!("THROUGHPUT {:.1} MiB/s", (SIZE / MIB) as f64 / secs);
    Ok(())
}
