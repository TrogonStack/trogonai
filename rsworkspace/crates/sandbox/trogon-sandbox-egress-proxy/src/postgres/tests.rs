use super::*;

fn ssl_request_bytes() -> Vec<u8> {
    let mut buf = Vec::with_capacity(SSL_REQUEST_LEN);
    buf.extend_from_slice(&(SSL_REQUEST_LEN as u32).to_be_bytes());
    buf.extend_from_slice(&SSL_REQUEST_CODE.to_be_bytes());
    buf
}

fn gss_enc_request_bytes() -> Vec<u8> {
    let mut buf = Vec::with_capacity(SSL_REQUEST_LEN);
    buf.extend_from_slice(&(SSL_REQUEST_LEN as u32).to_be_bytes());
    buf.extend_from_slice(&GSS_ENC_REQUEST_CODE.to_be_bytes());
    buf
}

#[test]
fn recognizes_well_formed_ssl_request() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(classify_pg_probe(&ssl_request_bytes()), PgProbe::SslRequest);
    Ok(())
}

#[test]
fn recognizes_well_formed_gss_enc_request() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(classify_pg_probe(&gss_enc_request_bytes()), PgProbe::GssEncRequest);
    Ok(())
}

#[test]
fn rejects_wrong_length() -> Result<(), Box<dyn std::error::Error>> {
    let mut bytes = ssl_request_bytes();
    bytes.push(0);
    assert_eq!(classify_pg_probe(&bytes), PgProbe::NotPostgres);
    Ok(())
}

#[test]
fn rejects_wrong_code() -> Result<(), Box<dyn std::error::Error>> {
    let mut bytes = ssl_request_bytes();
    bytes[4..8].copy_from_slice(&0u32.to_be_bytes());
    assert_eq!(classify_pg_probe(&bytes), PgProbe::NotPostgres);
    Ok(())
}

#[test]
fn rejects_ordinary_http_prefix() -> Result<(), Box<dyn std::error::Error>> {
    let bytes = b"GET / HT".to_vec();
    assert_eq!(classify_pg_probe(&bytes), PgProbe::NotPostgres);
    Ok(())
}

use tokio::net::TcpListener;

async fn socket_pair() -> Result<(TcpStream, TcpStream), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let peer = TcpStream::connect(listener.local_addr()?).await?;
    let (accepted, _) = listener.accept().await?;
    Ok((accepted, peer))
}

/// Runs the negotiation against an upstream that answers `response`, with a
/// client that sends `hello` and then closes it when `close` is set.
async fn negotiate_with(
    response: u8,
    hello: &[u8],
    close: bool,
) -> Result<(PgHandshakeOutcome, Vec<u8>), Box<dyn std::error::Error>> {
    let (mut client, mut peer) = socket_pair().await?;
    let (mut upstream, mut fake_upstream) = socket_pair().await?;
    let fake = tokio::spawn(async move {
        let mut request = [0u8; SSL_REQUEST_LEN];
        fake_upstream.read_exact(&mut request).await?;
        fake_upstream.write_all(&[response]).await?;
        let mut rest = Vec::new();
        fake_upstream.read_to_end(&mut rest).await?;
        io::Result::Ok(())
    });
    peer.write_all(hello).await?;
    if close {
        peer.shutdown().await?;
    }
    let outcome = negotiate(&mut client, &mut upstream, &ssl_request_bytes()).await?;
    drop((upstream, peer));
    fake.await??;
    Ok(outcome)
}

fn record(body_len: usize, sent: usize) -> Vec<u8> {
    let mut bytes = vec![22u8, 3, 1];
    bytes.extend_from_slice(&u16::try_from(body_len).unwrap_or(u16::MAX).to_be_bytes());
    bytes.resize(RECORD_HEADER_LEN + sent, 0);
    bytes
}

#[tokio::test]
async fn a_prefix_cut_short_by_the_client_is_returned_as_read() -> Result<(), Box<dyn std::error::Error>> {
    let (mut client, mut peer) = socket_pair().await?;
    peer.write_all(b"abc").await?;
    peer.shutdown().await?;
    let (bytes, probe) = peek_pg_probe(&mut client).await?;
    assert_eq!(bytes, b"abc");
    assert_eq!(probe, PgProbe::NotPostgres);
    Ok(())
}

#[tokio::test]
async fn an_upstream_that_declines_or_answers_oddly_leaves_the_connection_plaintext()
-> Result<(), Box<dyn std::error::Error>> {
    for response in [TLS_DECLINE, TLS_ERROR, b'X'] {
        let (outcome, relayed) = negotiate_with(response, b"", true).await?;
        assert_eq!(outcome, PgHandshakeOutcome::PlaintextDeclined);
        assert_eq!(relayed, ssl_request_bytes());
    }
    Ok(())
}

#[tokio::test]
async fn an_accepted_upgrade_relays_exactly_one_record() -> Result<(), Box<dyn std::error::Error>> {
    let hello = record(10, 10);
    let mut sent = hello.clone();
    sent.extend_from_slice(b"beyond the record");
    let (outcome, relayed) = negotiate_with(TLS_ACCEPT, &sent, true).await?;
    assert_eq!(outcome, PgHandshakeOutcome::TlsUpgraded { sni: None });
    assert_eq!(&relayed[SSL_REQUEST_LEN..], hello.as_slice());
    Ok(())
}

#[tokio::test]
async fn a_client_that_closes_inside_the_record_header_is_relayed_as_read() -> Result<(), Box<dyn std::error::Error>> {
    let (outcome, relayed) = negotiate_with(TLS_ACCEPT, &[22, 3], true).await?;
    assert_eq!(outcome, PgHandshakeOutcome::TlsUpgraded { sni: None });
    assert_eq!(&relayed[SSL_REQUEST_LEN..], &[22, 3]);
    Ok(())
}

#[tokio::test]
async fn a_record_larger_than_the_sniff_bound_is_relayed_as_its_header_only() -> Result<(), Box<dyn std::error::Error>>
{
    let (outcome, relayed) = negotiate_with(TLS_ACCEPT, &record(MAX_SNIFF_BYTES, 0), true).await?;
    assert_eq!(outcome, PgHandshakeOutcome::TlsUpgraded { sni: None });
    assert_eq!(relayed.len(), SSL_REQUEST_LEN + RECORD_HEADER_LEN);
    Ok(())
}

#[tokio::test]
async fn a_client_that_closes_inside_the_record_is_relayed_as_read() -> Result<(), Box<dyn std::error::Error>> {
    let (outcome, relayed) = negotiate_with(TLS_ACCEPT, &record(100, 4), true).await?;
    assert_eq!(outcome, PgHandshakeOutcome::TlsUpgraded { sni: None });
    assert_eq!(relayed.len(), SSL_REQUEST_LEN + RECORD_HEADER_LEN + 4);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_client_that_never_sends_its_hello_is_relayed_nothing_more() -> Result<(), Box<dyn std::error::Error>> {
    let (outcome, relayed) = negotiate_with(TLS_ACCEPT, b"", false).await?;
    assert_eq!(outcome, PgHandshakeOutcome::TlsUpgraded { sni: None });
    assert_eq!(relayed, ssl_request_bytes());
    Ok(())
}
