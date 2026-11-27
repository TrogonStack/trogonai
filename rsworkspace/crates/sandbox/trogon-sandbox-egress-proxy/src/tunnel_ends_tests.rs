//! Every way a terminated tunnel can end, over real TLS on both legs: each one
//! frees the tunnel,
//! its upstream connection and the bytes counted for it within a bounded
//! time, whatever its client or its upstream does or leaves undone.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{RwLockReadGuard, watch};
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout};
use tokio_rustls::TlsAcceptor;

use crate::bulk_transfers::{self, bounded};
use crate::dial::UpstreamMarking;
use crate::substitution_tests::{TestResult, lock};
use crate::tls_ca::{PermittedDomains, SandboxCa, install_default_crypto_provider};
use crate::tls_intercept::tests::{build_test_upstream_ca_and_leaf, client_trusting, root_store_from_pem, vetted};
use crate::tls_intercept::{PrefixedStream, TlsInterceptError, terminate_and_forward_checked};
use crate::tunnel_deadlines::{ByteBudget, SharedQueueBudget, TunnelDeadlines};

const HOST: &str = "api.example.com";
const MIB: usize = 1 << 20;
/// More than a proxy that stops reading, and the kernel, take in.
const UPLOAD: usize = 24 * MIB;
/// A tunnel still open this long after it should have closed counts as open.
const GIVE_UP: Duration = Duration::from_secs(3);
/// A timeout that plays no part in a scenario.
const NEVER: Duration = Duration::from_secs(600);
/// Timers do not fire early, but the two ends do not start timing at once.
const EARLY: Duration = Duration::from_millis(100);
/// A busy machine: how much later than its time a tunnel may close.
const LATE: Duration = Duration::from_secs(30);
/// How long a case polls for something that must happen; it returns the moment
/// it does, so only a failing case waits this long.
const PATIENCE: Duration = Duration::from_secs(60);
/// Longer than any pause an upstream in these scenarios takes.
const STOPPED: Duration = Duration::from_secs(20);

type ClientTls = tokio_rustls::client::TlsStream<TcpStream>;
type UpstreamTls = tokio_rustls::server::TlsStream<TcpStream>;

/// The tunnel timeouts; `keep_alive` is the keep-alive time and margin
/// together, and `stalled` bounds a tunnel under way in either direction.
#[derive(Clone, Copy)]
struct Timeouts {
    first_request: Duration,
    keep_alive: Duration,
    stalled: Duration,
}

const NO_TIMEOUTS: Timeouts = Timeouts {
    first_request: NEVER,
    keep_alive: NEVER,
    stalled: NEVER,
};

impl Timeouts {
    fn deadlines(self) -> TunnelDeadlines {
        TunnelDeadlines {
            handshake: Duration::from_secs(300),
            first_request: self.first_request,
            keep_alive: self.keep_alive,
            idle: self.stalled,
            stalled: self.stalled,
            ..TunnelDeadlines::default()
        }
    }
}

/// What the upstream does with a POST once it has its head.
#[derive(Clone, Copy)]
enum OnPost {
    /// Reads the body and answers 200 once it is all in.
    Read,
    /// Reads `burst`, then nothing for `pause`, and so on; 200 at the end.
    ReadInBursts { burst: usize, pause: Duration },
    /// Takes nothing more.
    Ignore,
    /// Answers 413 after 200 ms and takes nothing more.
    AnswerEarly,
}

#[derive(Clone, Copy)]
struct Behavior {
    on_post: OnPost,
    get_after: Duration,
}

impl Behavior {
    const PROMPT: Self = Self {
        on_post: OnPost::Read,
        get_after: Duration::ZERO,
    };

    fn on_post(on_post: OnPost) -> Self {
        Self {
            on_post,
            ..Self::PROMPT
        }
    }
}

struct Upstream {
    addr: std::net::SocketAddr,
    pem: String,
    open: Arc<AtomicUsize>,
    resume: watch::Sender<bool>,
}

struct Counted(Arc<AtomicUsize>);

impl Drop for Counted {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Upstream {
    async fn start(behavior: Behavior) -> TestResult<Self> {
        install_default_crypto_provider();
        let ca = build_test_upstream_ca_and_leaf(HOST)?;
        let acceptor = TlsAcceptor::from(Arc::new(
            rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(vec![ca.cert.clone()], ca.key.clone_key())?,
        ));
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let open = Arc::new(AtomicUsize::new(0));
        let (resume, resumed) = watch::channel(false);
        let counter = Arc::clone(&open);
        tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                counter.fetch_add(1, Ordering::SeqCst);
                let counted = Counted(Arc::clone(&counter));
                let acceptor = acceptor.clone();
                let resumed = resumed.clone();
                tokio::spawn(async move {
                    let _counted = counted;
                    if let Ok(tls) = acceptor.accept(tcp).await {
                        serve(tls, behavior, resumed).await;
                    }
                });
            }
        });
        Ok(Self {
            addr,
            pem: ca.pem,
            open,
            resume,
        })
    }

    /// The connections the proxy still holds: one left unread reads on, so
    /// that it sees its end.
    async fn still_open(&self) -> usize {
        let _ = self.resume.send(true);
        let until = Instant::now() + Duration::from_secs(2);
        while self.open.load(Ordering::SeqCst) > 0 && Instant::now() < until {
            sleep(Duration::from_millis(10)).await;
        }
        self.open.load(Ordering::SeqCst)
    }
}

async fn read_head(tls: &mut UpstreamTls, seen: &mut Vec<u8>) -> Option<(String, usize)> {
    let mut chunk = [0u8; 16 * 1024];
    loop {
        if let Some(at) = seen.windows(4).position(|window| window == b"\r\n\r\n") {
            let head: Vec<u8> = seen.drain(..at + 4).collect();
            return Some((String::from_utf8_lossy(&head).into_owned(), seen.len()));
        }
        match tls.read(&mut chunk).await {
            Ok(n) if n > 0 => seen.extend_from_slice(&chunk[..n]),
            _ => return None,
        }
    }
}

fn content_length(head: &str) -> usize {
    let lowered = head.to_ascii_lowercase();
    lowered
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .map_or(0, |value| value.trim().parse().map_or(0, |length| length))
}

async fn answer(tls: &mut UpstreamTls, status: u16, body: &str) -> bool {
    let response = format!("HTTP/1.1 {status} X\r\nContent-Length: {}\r\n\r\n{body}", body.len());
    tls.write_all(response.as_bytes()).await.is_ok()
}

async fn serve(mut tls: UpstreamTls, behavior: Behavior, mut resumed: watch::Receiver<bool>) {
    let mut seen = Vec::new();
    let mut chunk = vec![0u8; 64 * 1024];
    while let Some((head, mut taken)) = read_head(&mut tls, &mut seen).await {
        seen.clear();
        if !head.starts_with("POST") {
            sleep(behavior.get_after).await;
            if !answer(&mut tls, 200, "ok").await {
                return;
            }
            continue;
        }
        let length = content_length(&head);
        match behavior.on_post {
            OnPost::Read | OnPost::ReadInBursts { .. } => {
                let mut burst = 0;
                while taken < length {
                    let Ok(n @ 1..) = tls.read(&mut chunk).await else {
                        return;
                    };
                    taken += n;
                    burst += n;
                    if let OnPost::ReadInBursts { burst: most, pause } = behavior.on_post
                        && burst >= most
                    {
                        burst = 0;
                        sleep(pause).await;
                    }
                }
                if !answer(&mut tls, 200, "ok").await {
                    return;
                }
            }
            OnPost::Ignore => break,
            OnPost::AnswerEarly => {
                sleep(Duration::from_millis(200)).await;
                let _ = answer(&mut tls, 413, "early").await;
                break;
            }
        }
    }
    let _ = resumed.wait_for(|resume| *resume).await;
    while let Ok(Ok(1..)) = timeout(GIVE_UP, tls.read(&mut chunk)).await {}
}

/// Where the tunnel's CONNECT goes.
enum Target {
    Upstream(Upstream),
    /// Nothing listens there.
    Closed,
}

struct Setup {
    target: Target,
    timeouts: Timeouts,
    trust_upstream: bool,
}

impl Setup {
    fn to(target: Target, timeouts: Timeouts) -> Self {
        Self {
            target,
            timeouts,
            trust_upstream: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum End {
    CloseNotify,
    Abrupt,
}

struct Seen {
    text: Vec<u8>,
    last_byte_at: Instant,
    closed: Option<(Instant, End)>,
}

#[derive(Debug)]
struct Answer {
    status: u16,
}

#[derive(Debug)]
struct Ended {
    closed_after: Option<Duration>,
    close_notify: bool,
    proxy_done: bool,
    held: ByteBudget,
    upstream: Option<usize>,
}

impl Ended {
    /// The tunnel closed `least` (to `most`) on, and nothing of it is left.
    fn expect_closed(&self, least: Duration, most: Duration) -> TestResult<()> {
        let closed = self.closed_after.ok_or("the tunnel was never closed")?;
        assert!(closed + EARLY >= least, "closed after {closed:?}: {self:?}");
        assert!(closed < most + LATE, "closed after {closed:?}: {self:?}");
        assert!(self.proxy_done, "the proxy still holds the tunnel: {self:?}");
        assert_eq!(self.held, ByteBudget::from_bytes(0), "{self:?}");
        if let Some(upstream) = self.upstream {
            assert_eq!(upstream, 0, "the upstream connection is held: {self:?}");
        }
        Ok(())
    }
}

struct Tunnel {
    writer: WriteHalf<ClientTls>,
    seen: Arc<Mutex<Seen>>,
    proxy: JoinHandle<Result<(u64, u64), TlsInterceptError>>,
    budget: SharedQueueBudget,
    upstream: Option<Upstream>,
    authority: String,
    handshake_took: Duration,
    _timed: RwLockReadGuard<'static, ()>,
}

async fn open(setup: Setup) -> TestResult<Tunnel> {
    let timed = bulk_transfers::timed().await?;
    install_default_crypto_provider();
    let (addr, upstream_pem, upstream) = match setup.target {
        Target::Upstream(upstream) => (upstream.addr, upstream.pem.clone(), Some(upstream)),
        Target::Closed => ("127.0.0.1:1".parse()?, build_test_upstream_ca_and_leaf(HOST)?.pem, None),
    };
    let roots = match setup.trust_upstream {
        true => root_store_from_pem(&upstream_pem)?,
        false => root_store_from_pem(&build_test_upstream_ca_and_leaf(HOST)?.pem)?,
    };
    let upstream_tls = Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth(),
    );
    let sandbox_ca = SandboxCa::generate(PermittedDomains::for_tests(&["*.example.com"]))?;
    let connector = client_trusting(&sandbox_ca)?;
    let budget = SharedQueueBudget::new(ByteBudget::from_bytes(256 * MIB));
    let proxy_listener = TcpListener::bind("127.0.0.1:0").await?;
    let proxy_addr = proxy_listener.local_addr()?;
    let deadlines = setup.timeouts.deadlines();
    let shared = budget.clone();
    let proxy = tokio::spawn(async move {
        let (client_tcp, _) = proxy_listener
            .accept()
            .await
            .map_err(TlsInterceptError::ClientHandshake)?;
        terminate_and_forward_checked(
            PrefixedStream::new(client_tcp, vec![]),
            HOST,
            &sandbox_ca,
            vetted(addr).into(),
            UpstreamMarking::NotNeeded,
            upstream_tls,
            None,
            None,
            None,
            deadlines,
            None,
            None,
            Some(shared),
            None,
        )
        .await
    });
    let hello = Instant::now();
    let tcp = bounded("connecting to the proxy", TcpStream::connect(proxy_addr)).await??;
    let client = bounded(
        "the client handshake",
        connector.connect(rustls::pki_types::ServerName::try_from(HOST)?, tcp),
    )
    .await??;
    let (reader, writer) = tokio::io::split(client);
    let seen = Arc::new(Mutex::new(Seen {
        text: Vec::new(),
        last_byte_at: Instant::now(),
        closed: None,
    }));
    tokio::spawn(read_all(reader, Arc::clone(&seen)));
    Ok(Tunnel {
        writer,
        seen,
        proxy,
        budget,
        upstream,
        authority: format!("{HOST}:{}", addr.port()),
        handshake_took: hello.elapsed(),
        _timed: timed,
    })
}

async fn read_all(mut reader: ReadHalf<ClientTls>, seen: Arc<Mutex<Seen>>) {
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut chunk).await;
        let Ok(mut seen) = seen.lock() else {
            return;
        };
        match read {
            Ok(n @ 1..) => {
                seen.text.extend_from_slice(&chunk[..n]);
                seen.last_byte_at = Instant::now();
            }
            Ok(_) => {
                seen.closed = Some((Instant::now(), End::CloseNotify));
                return;
            }
            Err(_) => {
                seen.closed = Some((Instant::now(), End::Abrupt));
                return;
            }
        }
    }
}

fn whole_answer(text: &mut Vec<u8>) -> Option<Answer> {
    let at = text.windows(4).position(|window| window == b"\r\n\r\n")? + 4;
    let head = String::from_utf8_lossy(&text[..at]).into_owned();
    let length = content_length(&head);
    if text.len() < at + length {
        return None;
    }
    text.drain(..at + length);
    let status = head.split(' ').nth(1)?.parse().ok()?;
    Some(Answer { status })
}

impl Tunnel {
    /// How soon after the handshake a tunnel timed from its ClientHello may
    /// close: the floor, lowered by however much of `time` the handshake
    /// itself took on a busy runner.
    fn earliest_close(&self, floor: Duration, time: Duration) -> Duration {
        floor.min(time.saturating_sub(self.handshake_took))
    }

    /// The next whole answer (status 0: none came).
    async fn answer(seen: &Mutex<Seen>) -> TestResult<Answer> {
        let until = Instant::now() + PATIENCE;
        loop {
            {
                let mut seen = lock(seen)?;
                if let Some(answer) = whole_answer(&mut seen.text) {
                    return Ok(answer);
                }
                if seen.closed.is_some() || Instant::now() > until {
                    return Ok(Answer { status: 0 });
                }
            }
            sleep(Duration::from_millis(5)).await;
        }
    }

    async fn write(writer: &mut WriteHalf<ClientTls>, bytes: &[u8]) -> TestResult<()> {
        bounded("writing to the tunnel", writer.write_all(bytes)).await??;
        bounded("flushing the tunnel", writer.flush()).await??;
        Ok(())
    }

    async fn get(&mut self) -> TestResult<Answer> {
        let request = format!("GET / HTTP/1.1\r\nHost: {}\r\n\r\n", self.authority);
        Self::write(&mut self.writer, request.as_bytes()).await?;
        Self::answer(&self.seen).await
    }

    /// Sends a POST of `size`: true once it has all gone out, false once
    /// the tunnel takes nothing of it for [`STOPPED`]. The client pumps on
    /// behind its answer, so a pause that the upstream takes is waited out.
    async fn post(writer: &mut WriteHalf<ClientTls>, size: usize, host: &str, path: &str) -> bool {
        let head = format!("POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Length: {size}\r\n\r\n");
        if writer.write_all(head.as_bytes()).await.is_err() {
            return false;
        }
        let chunk = vec![7u8; 64 * 1024];
        for _ in 0..size / chunk.len() {
            let wrote = timeout(STOPPED, async {
                writer.write_all(&chunk).await?;
                writer.flush().await
            });
            match wrote.await {
                Ok(Ok(())) => {}
                _ => return false,
            }
        }
        true
    }

    async fn leave(&mut self) {
        let _ = timeout(GIVE_UP, self.writer.shutdown()).await;
    }

    fn last_byte_at(&self) -> TestResult<Instant> {
        Ok(lock(&self.seen)?.last_byte_at)
    }

    /// How the tunnel ends: when, from the proxy's last byte or from
    /// `since`; whether the proxy said so first; whether the tunnel, its
    /// upstream connection and the bytes counted for it are free again.
    async fn end(self, since: Option<Instant>) -> TestResult<Ended> {
        let since = match since {
            Some(since) => since,
            None => self.last_byte_at()?,
        };
        let until = Instant::now() + PATIENCE;
        let closed = loop {
            let closed = lock(&self.seen)?.closed;
            if let Some(closed) = closed {
                break Some(closed);
            }
            if Instant::now() > until {
                break None;
            }
            sleep(Duration::from_millis(5)).await;
        };
        drop(self.writer);
        let proxy_done = timeout(PATIENCE, self.proxy).await.is_ok();
        let upstream = match &self.upstream {
            Some(upstream) => Some(upstream.still_open().await),
            None => None,
        };
        Ok(Ended {
            closed_after: closed.map(|(at, _)| at.saturating_duration_since(since)),
            close_notify: matches!(closed, Some((_, End::CloseNotify))),
            proxy_done,
            held: self.budget.used(),
            upstream,
        })
    }
}

#[tokio::test]
async fn no_request_after_the_handshake_closes_the_tunnel_at_the_first_request_time()
-> Result<(), Box<dyn std::error::Error>> {
    let timeouts = Timeouts {
        first_request: Duration::from_millis(800),
        ..NO_TIMEOUTS
    };
    let tunnel = open(Setup::to(Target::Closed, timeouts)).await?;
    let least = tunnel.earliest_close(Duration::from_millis(500), timeouts.first_request);
    let since = Instant::now();
    let ended = tunnel.end(Some(since)).await?;
    ended.expect_closed(least, Duration::from_millis(800))?;
    Ok(())
}

#[tokio::test]
async fn half_a_head_after_the_handshake_closes_the_tunnel_at_the_first_request_time()
-> Result<(), Box<dyn std::error::Error>> {
    let timeouts = Timeouts {
        first_request: Duration::from_millis(800),
        ..NO_TIMEOUTS
    };
    let mut tunnel = open(Setup::to(Target::Closed, timeouts)).await?;
    let least = tunnel.earliest_close(Duration::from_millis(500), timeouts.first_request);
    let since = Instant::now();
    Tunnel::write(&mut tunnel.writer, b"GET / HTTP/1.1\r\nX-Half").await?;
    let ended = tunnel.end(Some(since)).await?;
    ended.expect_closed(least, Duration::from_millis(800))?;
    Ok(())
}

#[tokio::test]
async fn half_a_head_after_a_response_closes_the_tunnel_at_the_keep_alive_time()
-> Result<(), Box<dyn std::error::Error>> {
    let timeouts = Timeouts {
        keep_alive: Duration::from_millis(800),
        ..NO_TIMEOUTS
    };
    let upstream = Upstream::start(Behavior::PROMPT).await?;
    let mut tunnel = open(Setup::to(Target::Upstream(upstream), timeouts)).await?;
    let since = Instant::now();
    assert_eq!(tunnel.get().await?.status, 200);
    Tunnel::write(&mut tunnel.writer, b"GET / HTTP/1.1\r\nX-Half").await?;
    let ended = tunnel.end(Some(since)).await?;
    ended.expect_closed(Duration::from_millis(800), Duration::from_millis(800))?;
    assert!(ended.close_notify, "{ended:?}");
    Ok(())
}

#[tokio::test]
async fn half_a_body_is_closed_once_it_has_not_moved_for_the_time_allowed() -> Result<(), Box<dyn std::error::Error>> {
    let timeouts = Timeouts {
        stalled: Duration::from_millis(500),
        ..NO_TIMEOUTS
    };
    let upstream = Upstream::start(Behavior::PROMPT).await?;
    let mut tunnel = open(Setup::to(Target::Upstream(upstream), timeouts)).await?;
    let since = Instant::now();
    let head = format!(
        "POST / HTTP/1.1\r\nHost: {}\r\nContent-Length: 10\r\n\r\nhalf",
        tunnel.authority
    );
    Tunnel::write(&mut tunnel.writer, head.as_bytes()).await?;
    let ended = tunnel.end(Some(since)).await?;
    ended.expect_closed(Duration::from_millis(500), Duration::from_millis(1000))?;
    Ok(())
}

#[tokio::test]
async fn a_body_that_trickles_in_arrives() -> Result<(), Box<dyn std::error::Error>> {
    let timeouts = Timeouts {
        stalled: Duration::from_millis(2500),
        ..NO_TIMEOUTS
    };
    let upstream = Upstream::start(Behavior::PROMPT).await?;
    let mut tunnel = open(Setup::to(Target::Upstream(upstream), timeouts)).await?;
    let head = format!(
        "POST / HTTP/1.1\r\nHost: {}\r\nContent-Length: 8\r\n\r\n",
        tunnel.authority
    );
    Tunnel::write(&mut tunnel.writer, head.as_bytes()).await?;
    for _ in 0..8 {
        sleep(Duration::from_millis(100)).await;
        Tunnel::write(&mut tunnel.writer, b".").await?;
    }
    assert_eq!(Tunnel::answer(&tunnel.seen).await?.status, 200);
    Ok(())
}

#[tokio::test]
async fn a_whole_request_may_wait_for_its_answer_longer_than_every_timeout() -> Result<(), Box<dyn std::error::Error>> {
    let timeouts = Timeouts {
        first_request: Duration::from_secs(20),
        keep_alive: Duration::from_millis(300),
        stalled: Duration::from_millis(300),
    };
    let slow = Behavior {
        get_after: Duration::from_millis(1200),
        ..Behavior::PROMPT
    };
    let upstream = Upstream::start(slow).await?;
    let mut tunnel = open(Setup::to(Target::Upstream(upstream), timeouts)).await?;
    assert_eq!(tunnel.get().await?.status, 200);
    Ok(())
}

#[tokio::test]
async fn each_response_starts_the_idle_time_anew() -> Result<(), Box<dyn std::error::Error>> {
    let keep_alive = Duration::from_millis(2500);
    let timeouts = Timeouts {
        keep_alive,
        ..NO_TIMEOUTS
    };
    let upstream = Upstream::start(Behavior::PROMPT).await?;
    let mut tunnel = open(Setup::to(Target::Upstream(upstream), timeouts)).await?;
    for i in 0..3 {
        if i > 0 {
            sleep(Duration::from_millis(200)).await;
        }
        assert_eq!(tunnel.get().await?.status, 200);
    }
    let ended = tunnel.end(None).await?;
    ended.expect_closed(keep_alive, keep_alive)?;
    assert!(ended.close_notify, "{ended:?}");
    Ok(())
}

#[tokio::test]
async fn a_tunnel_is_idle_only_once_no_response_is_owed() -> Result<(), Box<dyn std::error::Error>> {
    let timeouts = Timeouts {
        keep_alive: Duration::from_millis(300),
        ..NO_TIMEOUTS
    };
    let slow = Behavior {
        get_after: Duration::from_millis(700),
        ..Behavior::PROMPT
    };
    let upstream = Upstream::start(slow).await?;
    let mut tunnel = open(Setup::to(Target::Upstream(upstream), timeouts)).await?;
    let request = format!("GET / HTTP/1.1\r\nHost: {}\r\n\r\n", tunnel.authority);
    Tunnel::write(&mut tunnel.writer, request.repeat(2).as_bytes()).await?;
    assert_eq!(Tunnel::answer(&tunnel.seen).await?.status, 200);
    assert_eq!(Tunnel::answer(&tunnel.seen).await?.status, 200);
    let ended = tunnel.end(None).await?;
    ended.expect_closed(Duration::from_millis(300), Duration::from_millis(300))?;
    assert!(ended.close_notify, "{ended:?}");
    Ok(())
}

#[tokio::test]
async fn a_tunnel_asked_again_when_the_advertised_time_is_up_serves_it() -> Result<(), Box<dyn std::error::Error>> {
    let timeouts = Timeouts {
        keep_alive: Duration::from_secs(4),
        ..NO_TIMEOUTS
    };
    let upstream = Upstream::start(Behavior::PROMPT).await?;
    let mut tunnel = open(Setup::to(Target::Upstream(upstream), timeouts)).await?;
    assert_eq!(tunnel.get().await?.status, 200);
    sleep(Duration::from_millis(1100)).await;
    assert_eq!(tunnel.get().await?.status, 200);
    Ok(())
}

#[tokio::test]
async fn a_tunnel_is_ended_by_its_own_keep_alive_time_alone() -> Result<(), Box<dyn std::error::Error>> {
    let keep_alive = Duration::from_millis(200 + 1800);
    let timeouts = Timeouts {
        keep_alive,
        ..NO_TIMEOUTS
    };
    let upstream = Upstream::start(Behavior::PROMPT).await?;
    let mut tunnel = open(Setup::to(Target::Upstream(upstream), timeouts)).await?;
    assert_eq!(tunnel.get().await?.status, 200);
    let ended = tunnel.end(None).await?;
    ended.expect_closed(keep_alive, keep_alive)?;
    assert!(ended.close_notify, "{ended:?}");
    Ok(())
}

#[tokio::test]
async fn an_idle_tunnel_whose_client_answers_no_end_frees_its_slot_within_as_long_again()
-> Result<(), Box<dyn std::error::Error>> {
    let keep_alive = Duration::from_millis(500);
    let timeouts = Timeouts {
        keep_alive,
        ..NO_TIMEOUTS
    };
    let upstream = Upstream::start(Behavior::PROMPT).await?;
    let mut tunnel = open(Setup::to(Target::Upstream(upstream), timeouts)).await?;
    assert_eq!(tunnel.get().await?.status, 200);
    let since = tunnel.last_byte_at()?;
    timeout(PATIENCE, &mut tunnel.proxy).await???;
    let free_after = since.elapsed();
    assert!(free_after + EARLY >= keep_alive, "freed after {free_after:?}");
    assert!(free_after < 2 * keep_alive + LATE, "freed after {free_after:?}");
    Ok(())
}

#[tokio::test]
async fn a_stalled_tunnel_is_closed_between_one_and_one_and_a_fifth_times_the_time_allowed()
-> Result<(), Box<dyn std::error::Error>> {
    let stalled = Duration::from_millis(1000);
    let timeouts = Timeouts { stalled, ..NO_TIMEOUTS };
    let upstream = Upstream::start(Behavior::PROMPT).await?;
    let mut tunnel = open(Setup::to(Target::Upstream(upstream), timeouts)).await?;
    let head = format!(
        "POST / HTTP/1.1\r\nHost: {}\r\nContent-Length: 10\r\n\r\nhalf",
        tunnel.authority
    );
    Tunnel::write(&mut tunnel.writer, head.as_bytes()).await?;
    sleep(Duration::from_millis(20)).await;
    Tunnel::write(&mut tunnel.writer, b".").await?;
    let since = Instant::now();
    let ended = tunnel.end(Some(since)).await?;
    ended.expect_closed(stalled, stalled + stalled / 5)?;
    Ok(())
}

#[tokio::test]
async fn a_tunnel_whose_client_has_gone_is_not_ended_by_the_proxy() -> Result<(), Box<dyn std::error::Error>> {
    for answered in [false, true] {
        let upstream = Upstream::start(Behavior::PROMPT).await?;
        let mut tunnel = open(Setup::to(Target::Upstream(upstream), NO_TIMEOUTS)).await?;
        if answered {
            assert_eq!(tunnel.get().await?.status, 200);
        }
        tunnel.leave().await;
        let done = timeout(PATIENCE, &mut tunnel.proxy).await??;
        assert!(done.is_ok(), "answered {answered}: {done:?}");
    }
    Ok(())
}

/// An upload answered before it is all in, whose client then leaves (or,
/// with `stays`, does nothing more).
async fn answered_early(setup: Setup, host: Option<&str>, path: &str, stays: bool) -> TestResult<(u16, Ended)> {
    let mut tunnel = open(setup).await?;
    let host = host.map_or_else(|| tunnel.authority.clone(), str::to_owned);
    let Tunnel { writer, seen, .. } = &mut tunnel;
    let (_, answer) = tokio::join!(Tunnel::post(writer, UPLOAD, &host, path), Tunnel::answer(seen));
    let answer = answer?;
    if !stays {
        tunnel.leave().await;
    }
    Ok((answer.status, tunnel.end(None).await?))
}

async fn answering_early() -> TestResult<Target> {
    Ok(Target::Upstream(
        Upstream::start(Behavior::on_post(OnPost::AnswerEarly)).await?,
    ))
}

#[tokio::test]
async fn an_upload_answered_early_whose_client_leaves() -> Result<(), Box<dyn std::error::Error>> {
    let setup = Setup::to(answering_early().await?, NO_TIMEOUTS);
    let (status, ended) = answered_early(setup, None, "/", false).await?;
    assert_eq!(status, 413, "{ended:?}");
    ended.expect_closed(EARLY, EARLY)?;
    Ok(())
}

#[tokio::test]
async fn an_upload_whose_upstream_port_is_closed_whose_client_leaves() -> Result<(), Box<dyn std::error::Error>> {
    let setup = Setup::to(Target::Closed, NO_TIMEOUTS);
    let (status, ended) = answered_early(setup, None, "/", false).await?;
    assert_eq!(status, 502, "{ended:?}");
    ended.expect_closed(EARLY, EARLY)?;
    Ok(())
}

#[tokio::test]
async fn an_upload_whose_upstream_certificate_is_untrusted_whose_client_leaves()
-> Result<(), Box<dyn std::error::Error>> {
    let setup = Setup {
        trust_upstream: false,
        ..Setup::to(answering_early().await?, NO_TIMEOUTS)
    };
    let (status, ended) = answered_early(setup, None, "/", false).await?;
    assert_eq!(status, 502, "{ended:?}");
    ended.expect_closed(EARLY, EARLY)?;
    Ok(())
}

#[tokio::test]
async fn an_upload_refused_for_its_host_whose_client_leaves() -> Result<(), Box<dyn std::error::Error>> {
    let setup = Setup::to(answering_early().await?, NO_TIMEOUTS);
    let (status, ended) = answered_early(setup, Some("other.test"), "/", false).await?;
    assert_eq!(status, 421, "{ended:?}");
    ended.expect_closed(EARLY, EARLY)?;
    Ok(())
}

#[tokio::test]
async fn an_upload_refused_for_its_target_whose_client_leaves() -> Result<(), Box<dyn std::error::Error>> {
    let setup = Setup::to(answering_early().await?, NO_TIMEOUTS);
    let (status, ended) = answered_early(setup, None, "*", false).await?;
    assert_eq!(status, 400, "{ended:?}");
    ended.expect_closed(EARLY, EARLY)?;
    Ok(())
}

#[tokio::test]
async fn an_upload_answered_early_whose_client_stays_is_idle_from_the_answer_on()
-> Result<(), Box<dyn std::error::Error>> {
    let timeouts = Timeouts {
        keep_alive: Duration::from_millis(600),
        ..NO_TIMEOUTS
    };
    let setup = Setup::to(answering_early().await?, timeouts);
    let (status, ended) = answered_early(setup, None, "/", true).await?;
    assert_eq!(status, 413, "{ended:?}");
    ended.expect_closed(EARLY, Duration::from_millis(600))?;
    assert!(ended.close_notify, "{ended:?}");
    Ok(())
}

#[tokio::test]
async fn an_upload_answered_early_whose_client_sends_it_all_has_its_next_request_served()
-> Result<(), Box<dyn std::error::Error>> {
    let mut tunnel = open(Setup::to(answering_early().await?, NO_TIMEOUTS)).await?;
    let host = tunnel.authority.clone();
    let Tunnel { writer, seen, .. } = &mut tunnel;
    let (all_sent, answer) = tokio::join!(Tunnel::post(writer, UPLOAD, &host, "/"), Tunnel::answer(seen));
    assert_eq!(answer?.status, 413);
    assert!(all_sent, "the rest of the upload was not read on");
    assert_eq!(tunnel.get().await?.status, 200);
    tunnel.leave().await;
    let ended = tunnel.end(None).await?;
    assert_eq!(ended.held, ByteBudget::from_bytes(0), "{ended:?}");
    assert!(ended.proxy_done, "{ended:?}");
    Ok(())
}

#[tokio::test]
async fn an_upload_the_upstream_stops_taking_is_closed_once_it_has_not_moved() -> Result<(), Box<dyn std::error::Error>>
{
    let timeouts = Timeouts {
        stalled: Duration::from_millis(500),
        ..NO_TIMEOUTS
    };
    let upstream = Upstream::start(Behavior::on_post(OnPost::Ignore)).await?;
    let mut tunnel = open(Setup::to(Target::Upstream(upstream), timeouts)).await?;
    let since = Instant::now();
    let host = tunnel.authority.clone();
    Tunnel::post(&mut tunnel.writer, 2 * UPLOAD, &host, "/").await;
    let ended = tunnel.end(Some(since)).await?;
    ended.expect_closed(Duration::from_millis(500), Duration::from_millis(1000))?;
    assert!(!ended.close_notify, "{ended:?}");
    Ok(())
}

#[tokio::test]
async fn an_upload_the_upstream_takes_in_bursts_arrives() -> Result<(), Box<dyn std::error::Error>> {
    let timeouts = Timeouts {
        stalled: Duration::from_millis(1500),
        ..NO_TIMEOUTS
    };
    let bursts = OnPost::ReadInBursts {
        burst: 6 * MIB,
        pause: Duration::from_millis(400),
    };
    let upstream = Upstream::start(Behavior::on_post(bursts)).await?;
    let mut tunnel = open(Setup::to(Target::Upstream(upstream), timeouts)).await?;
    let host = tunnel.authority.clone();
    let all_sent = Tunnel::post(&mut tunnel.writer, 2 * UPLOAD, &host, "/").await;
    assert!(all_sent, "the upload stopped moving");
    assert_eq!(Tunnel::answer(&tunnel.seen).await?.status, 200);
    Ok(())
}
