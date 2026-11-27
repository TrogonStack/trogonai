use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufWriter, DuplexStream, duplex};

fn deadlines() -> TunnelDeadlines {
    TunnelDeadlines {
        handshake: Duration::from_secs(10),
        handshake_started: None,
        first_request: Duration::from_secs(10),
        idle: Duration::from_secs(60),
        stalled: Duration::from_secs(30),
        keep_alive: Duration::from_secs(6),
        max_queued_for_client: ByteBudget::from_kib(16),
    }
}

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn end_of(err: &io::Error) -> Result<TunnelEndError, Box<dyn std::error::Error>> {
    assert_eq!(err.kind(), io::ErrorKind::TimedOut);
    err.get_ref()
        .and_then(|source| source.downcast_ref::<TunnelEndError>())
        .copied()
        .ok_or_else(|| "expected a TunnelEndError source".into())
}

fn ended<T>(result: io::Result<T>) -> Result<io::Error, Box<dyn std::error::Error>> {
    match result {
        Ok(_) => Err("expected the tunnel to be cut off".into()),
        Err(err) => Ok(err),
    }
}

#[test]
fn defaults_are_ten_seconds_to_start_and_five_minutes_idle() -> TestResult {
    let defaults = TunnelDeadlines::default();
    assert_eq!(defaults.handshake, Duration::from_secs(10));
    assert_eq!(defaults.first_request, Duration::from_secs(10));
    assert_eq!(defaults.idle, Duration::from_secs(300));
    assert_eq!(defaults.stalled, Duration::from_secs(300));
    assert_eq!(defaults.max_queued_for_client.bytes(), 1 << 20);
    assert_eq!(defaults.keep_alive, Duration::from_secs(6));
    assert_eq!(SHARED_QUEUE_BUDGET.bytes(), 256 << 20);
    Ok(())
}

/// A tunnel whose writes queue until its client reads: what it was
/// handed goes out only as far as the client has taken it.
fn queued_tunnel(shared: &SharedQueueBudget) -> (DuplexStream, DeadlineStream<BufWriter<DuplexStream>>) {
    let (client, server) = duplex(1024);
    let tunnel = DeadlineStream::new(BufWriter::with_capacity(64 * 1024, server), deadlines())
        .with_shared_budget(shared.clone());
    (client, tunnel)
}

async fn drain(mut client: DuplexStream, bytes: usize) -> io::Result<()> {
    let mut sink = vec![0u8; bytes];
    client.read_exact(&mut sink).await?;
    Ok(())
}

#[tokio::test]
async fn a_tunnel_holding_queued_bytes_waits_while_the_shared_budget_is_spent() -> TestResult {
    let shared = SharedQueueBudget::new(ByteBudget::from_bytes(4096));
    let (client, mut slow) = queued_tunnel(&shared);
    slow.write_all(&[1u8; 8192]).await?;
    assert_eq!(shared.used().bytes(), 8192);

    let blocked = tokio::time::timeout(Duration::from_millis(50), slow.write_all(b"more"));
    assert!(blocked.await.is_err());

    let resumed = async {
        slow.write_all(b"more").await?;
        assert_eq!(shared.used().bytes(), 4);
        slow.flush().await
    };
    let (resumed, drained) = tokio::join!(resumed, drain(client, 8192 + 4));
    resumed?;
    drained?;
    assert_eq!(shared.used().bytes(), 0);
    Ok(())
}

#[tokio::test]
async fn tunnels_that_together_spent_the_budget_each_go_on_once_their_clients_read() -> TestResult {
    let shared = SharedQueueBudget::new(ByteBudget::from_bytes(4096));
    let (first_client, mut first) = queued_tunnel(&shared);
    let (second_client, mut second) = queued_tunnel(&shared);
    first.write_all(&[1u8; 3000]).await?;
    second.write_all(&[2u8; 3000]).await?;
    let body = [3u8; 3000];
    let first_goes_on = async {
        for _ in 0..10 {
            first.write_all(&body).await?;
        }
        first.flush().await
    };
    let second_goes_on = async {
        for _ in 0..10 {
            second.write_all(&body).await?;
        }
        second.flush().await
    };
    let (first_done, second_done, first_drained, second_drained) =
        tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(
                first_goes_on,
                second_goes_on,
                drain(first_client, 3000 * 11),
                drain(second_client, 3000 * 11),
            )
        })
        .await?;
    first_done?;
    second_done?;
    first_drained?;
    second_drained?;
    assert_eq!(shared.used().bytes(), 0);
    Ok(())
}

#[tokio::test]
async fn a_tunnel_holding_nothing_still_writes_when_the_shared_budget_is_spent() -> TestResult {
    let shared = SharedQueueBudget::new(ByteBudget::from_bytes(1024));
    let (_hog_client, hog_server) = duplex(64 * 1024);
    let mut hog = DeadlineStream::new(hog_server, deadlines()).with_shared_budget(shared.clone());
    hog.write_all(&[1u8; 4096]).await?;

    let (mut client, server) = duplex(1024);
    let mut other = DeadlineStream::new(server, deadlines()).with_shared_budget(shared.clone());
    other.write_all(b"hi").await?;
    let mut two = [0u8; 2];
    client.read_exact(&mut two).await?;
    assert_eq!(&two, b"hi");
    Ok(())
}

#[tokio::test]
async fn dropping_a_tunnel_returns_its_share_of_the_shared_budget() -> TestResult {
    let shared = SharedQueueBudget::new(ByteBudget::from_bytes(4096));
    let (_client, server) = duplex(64 * 1024);
    let mut tunnel = DeadlineStream::new(server, deadlines()).with_shared_budget(shared.clone());
    tunnel.write_all(&[1u8; 100]).await?;
    assert_eq!(shared.used().bytes(), 100);
    drop(tunnel);
    assert_eq!(shared.used().bytes(), 0);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_client_that_never_begins_a_request_is_cut_off() -> TestResult {
    let (client, server) = duplex(1024);
    let mut tunnel = DeadlineStream::new(server, deadlines());
    let started = Instant::now();
    let err = ended(tunnel.read(&mut [0u8; 16]).await)?;
    assert_eq!(end_of(&err)?, TunnelEndError::NoFirstRequest(Duration::from_secs(10)));
    assert_eq!(started.elapsed(), Duration::from_secs(10));
    drop(client);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_client_that_goes_quiet_after_a_request_is_cut_off_after_the_idle_time() -> TestResult {
    let (mut client, server) = duplex(1024);
    let mut tunnel = DeadlineStream::new(server, deadlines());
    client.write_all(b"x").await?;
    let mut one = [0u8; 1];
    tunnel.read_exact(&mut one).await?;
    let quiet_since = Instant::now();
    let err = ended(tunnel.read(&mut one).await)?;
    assert_eq!(end_of(&err)?, TunnelEndError::Idle(Duration::from_secs(60)));
    assert_eq!(quiet_since.elapsed(), Duration::from_secs(60));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn bytes_in_either_direction_keep_a_tunnel_open() -> TestResult {
    let (mut client, server) = duplex(1024);
    let mut tunnel = DeadlineStream::new(server, deadlines());
    let mut one = [0u8; 1];
    for _ in 0..4 {
        tokio::time::advance(Duration::from_secs(50)).await;
        client.write_all(b"x").await?;
        tunnel.read_exact(&mut one).await?;
    }
    for _ in 0..4 {
        tokio::time::advance(Duration::from_secs(50)).await;
        tunnel.write_all(b"y").await?;
        client.read_exact(&mut one).await?;
    }
    tokio::time::advance(Duration::from_secs(50)).await;
    client.write_all(b"z").await?;
    tunnel.read_exact(&mut one).await?;
    assert_eq!(&one, b"z");
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_client_that_takes_none_of_a_write_is_cut_off() -> TestResult {
    let (client, server) = duplex(1024);
    let mut tunnel = DeadlineStream::new(server, deadlines());
    let started = Instant::now();
    let err = ended(tunnel.write_all(&vec![0u8; 1 << 20]).await)?;
    assert_eq!(end_of(&err)?, TunnelEndError::ClientNotReading(Duration::from_secs(30)));
    assert_eq!(started.elapsed(), Duration::from_secs(30));
    drop(client);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_client_that_reads_in_bursts_loses_nothing() -> TestResult {
    let (mut client, server) = duplex(1024);
    let mut tunnel = DeadlineStream::new(server, deadlines());
    let payload = vec![7u8; 8 * 1024];
    let writer = tokio::spawn(async move {
        tunnel.write_all(&payload).await?;
        Ok::<_, io::Error>(tunnel)
    });
    let mut received = Vec::new();
    while received.len() < 8 * 1024 {
        tokio::time::advance(Duration::from_secs(20)).await;
        let mut chunk = [0u8; 1024];
        let n = client.read(&mut chunk).await?;
        received.extend_from_slice(&chunk[..n]);
    }
    assert_eq!(received, vec![7u8; 8 * 1024]);
    writer.await??;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_pending_write_is_not_mistaken_for_an_idle_read() -> TestResult {
    let (mut client, server) = duplex(64);
    let (mut read_half, mut write_half) = tokio::io::split(DeadlineStream::new(server, deadlines()));
    client.write_all(b"x").await?;
    let mut one = [0u8; 1];
    read_half.read_exact(&mut one).await?;
    let writer = tokio::spawn(async move { write_half.write_all(&[1u8; 4096]).await });
    let reader = tokio::spawn(async move { read_half.read(&mut [0u8; 1]).await });
    tokio::time::advance(Duration::from_secs(25)).await;
    let mut sink = vec![0u8; 4096];
    client.read_exact(&mut sink).await?;
    writer.await??;
    client.write_all(b"y").await?;
    assert_eq!(reader.await??, 1);
    Ok(())
}

#[test]
fn a_flush_end_is_worded_for_the_log() -> TestResult {
    assert_eq!(
        FlushEndError::ClientEndedAfterFlush.to_string(),
        "the client ending after the flush"
    );
    assert_eq!(
        FlushEndError::GraceAfterFlush.to_string(),
        "the 5 s grace after the flush"
    );
    assert_eq!(
        FlushEndError::NotFlushed(Duration::from_millis(400)).to_string(),
        "not flushed in 400 ms"
    );
    assert_eq!(FlushEndError::ClientClosed.to_string(), "the client closing");
    assert_eq!(GRACE_AFTER_FLUSH, Duration::from_secs(5));
    Ok(())
}

#[tokio::test]
async fn a_slow_client_that_sends_bytes_nobody_reads_still_gets_every_byte_then_an_end() -> TestResult {
    const SIZE: usize = 32 << 20;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await?;
        let chunk = vec![b'a'; 64 << 10];
        let mut sent = 0;
        while sent < SIZE {
            stream.write_all(&chunk).await?;
            sent += chunk.len();
        }
        Ok::<_, io::Error>(close_after_flush(&mut stream, STALLED_TIMEOUT, GRACE_AFTER_FLUSH).await)
    });
    let (mut read_half, write_half) = tokio::net::TcpStream::connect(addr).await?.into_split();
    let mut write_half = Some(write_half);
    let mut read = 0;
    let mut poke = None;
    let mut buf = vec![0u8; 64 << 10];
    loop {
        let n = read_half.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        read += n;
        if let Some(mut write_half) = write_half.take_if(|_| read > SIZE / 2) {
            poke = Some(tokio::spawn(async move {
                write_half.write_all(&vec![b'b'; 4 << 20]).await?;
                Ok::<_, io::Error>(write_half)
            }));
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    assert_eq!(read, SIZE);
    let mut write_half = poke.ok_or("expected a value")?.await??;
    write_half.shutdown().await?;
    assert_eq!(server.await??, FlushEndError::ClientEndedAfterFlush);
    Ok(())
}

#[tokio::test]
async fn a_client_that_takes_nothing_is_cut_after_the_time_allowed_not_before() -> TestResult {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let _client = tokio::net::TcpStream::connect(addr).await?;
    let (stream, _) = listener.accept().await?;
    let mut stream = tokio::io::BufWriter::with_capacity((32 << 20) + 1, stream);
    stream.write_all(&vec![b'a'; 32 << 20]).await?;
    let began = Instant::now();
    let end = close_after_flush(&mut stream, Duration::from_millis(400), GRACE_AFTER_FLUSH).await;
    assert_eq!(end, FlushEndError::NotFlushed(Duration::from_millis(400)));
    assert!(began.elapsed() >= Duration::from_millis(400));
    Ok(())
}

#[tokio::test]
async fn a_client_that_never_ends_is_closed_after_the_grace() -> TestResult {
    tokio::time::pause();
    let (_client, mut server) = duplex(1024);
    assert_eq!(
        close_after_flush(&mut server, STALLED_TIMEOUT, GRACE_AFTER_FLUSH).await,
        FlushEndError::GraceAfterFlush
    );
    Ok(())
}

/// A stream whose writes, flushes and shutdowns fail as scripted and whose
/// reads either fail with a set kind or never arrive.
struct Faulty {
    read_error: Option<io::ErrorKind>,
    write_fails: bool,
    shutdown_pending: bool,
}

impl Faulty {
    fn failing() -> Self {
        Self {
            read_error: None,
            write_fails: true,
            shutdown_pending: false,
        }
    }

    fn accepting() -> Self {
        Self {
            write_fails: false,
            ..Self::failing()
        }
    }
}

impl AsyncRead for Faulty {
    fn poll_read(self: Pin<&mut Self>, _: &mut Context<'_>, _: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        match self.read_error {
            Some(kind) => Poll::Ready(Err(kind.into())),
            None => Poll::Pending,
        }
    }
}

impl AsyncWrite for Faulty {
    fn poll_write(self: Pin<&mut Self>, _: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        if self.write_fails {
            return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
        }
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.shutdown_pending {
            return Poll::Pending;
        }
        Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()))
    }
}

#[tokio::test]
async fn a_failed_write_reaches_the_caller_unchanged() -> TestResult {
    let mut tunnel = DeadlineStream::new(Faulty::failing(), deadlines());
    let err = ended(tunnel.write_all(b"x").await)?;
    assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
    Ok(())
}

#[tokio::test]
async fn a_failed_flush_while_the_shared_budget_is_spent_reaches_the_caller() -> TestResult {
    let shared = SharedQueueBudget::new(ByteBudget::from_bytes(1));
    let mut tunnel = DeadlineStream::new(Faulty::accepting(), deadlines()).with_shared_budget(shared);
    tunnel.write_all(b"held").await?;
    let err = ended(tunnel.write_all(b"more").await)?;
    assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
    Ok(())
}

#[tokio::test]
async fn shutting_a_tunnel_down_reaches_the_stream_beneath() -> TestResult {
    let (mut client, server) = duplex(64);
    let mut tunnel = DeadlineStream::new(server, deadlines());
    tunnel.shutdown().await?;
    let mut rest = Vec::new();
    client.read_to_end(&mut rest).await?;
    assert!(rest.is_empty());

    let mut failing = DeadlineStream::new(Faulty::failing(), deadlines());
    let err = ended(failing.shutdown().await)?;
    assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
    Ok(())
}

#[tokio::test]
async fn a_close_that_fails_ends_the_flush_as_a_closed_client() -> TestResult {
    let mut stream = Faulty::failing();
    let end = close_after_flush(&mut stream, Duration::from_secs(30), Duration::from_secs(5)).await;
    assert_eq!(end, FlushEndError::ClientClosed);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_client_that_ends_before_the_flush_finishes_does_not_end_it_early() -> TestResult {
    let mut stream = Faulty {
        read_error: Some(io::ErrorKind::UnexpectedEof),
        write_fails: false,
        shutdown_pending: true,
    };
    let end = close_after_flush(&mut stream, Duration::from_secs(30), Duration::from_secs(5)).await;
    assert_eq!(end, FlushEndError::NotFlushed(Duration::from_secs(30)));
    Ok(())
}
