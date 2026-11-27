use super::*;
use tokio::io::{AsyncWriteExt, duplex};
use trogon_sandbox_contract::{OpaqueTunnelPolicy::Allow, OpaqueTunnelPolicy::Refuse};

#[tokio::test]
async fn a_client_hello_is_terminable() -> Result<(), Box<dyn std::error::Error>> {
    let (mut peer, mut client) = duplex(64);
    peer.write_all(&[0x16, 3, 1]).await?;
    assert_eq!(
        first_bytes(&mut client, vec![], Duration::from_secs(1), Refuse).await,
        FirstBytes::Terminable(vec![0x16, 3, 1])
    );
    Ok(())
}

#[tokio::test]
async fn http_bytes_are_not_tls() -> Result<(), Box<dyn std::error::Error>> {
    let (mut peer, mut client) = duplex(64);
    peer.write_all(b"GET / HTTP/1.1\r\n").await?;
    assert_eq!(
        first_bytes(&mut client, vec![], Duration::from_secs(1), Refuse).await,
        FirstBytes::NotTls
    );
    Ok(())
}

#[tokio::test]
async fn already_read_bytes_are_judged_without_reading() -> Result<(), Box<dyn std::error::Error>> {
    let (_peer, mut client) = duplex(64);
    assert_eq!(
        first_bytes(&mut client, b"SSH-2.0".to_vec(), Duration::from_secs(1), Refuse).await,
        FirstBytes::NotTls
    );
    Ok(())
}

#[test]
fn a_record_header_is_judged_by_its_three_bytes() -> Result<(), Box<dyn std::error::Error>> {
    for header in [&[0x16, 3, 0][..], &[0x16, 3, 1], &[0x16, 3, 2], &[0x16, 3, 3]] {
        assert!(starts_like_tls(header), "{header:?}");
    }
    for header in [
        &[0x15, 3, 1, 0, 2][..],
        &[0x16, 2, 1],
        &[0x16, 3, 4],
        &[0x17, 3, 3],
        b"SSH-2.0-OpenSSH_9.7\r\n",
        b"GET / HTTP/1.1\r\n",
    ] {
        assert!(!starts_like_tls(header), "{header:?}");
    }
    Ok(())
}

#[test]
fn a_prefix_too_short_to_judge_is_never_judged_not_tls() -> Result<(), Box<dyn std::error::Error>> {
    for prefix in [&[][..], &[0x16], &[0x16, 3], &[0x16, 5], &[0x15], b"GE"] {
        assert!(!judged_not_tls(prefix), "{prefix:?}");
    }
    assert!(judged_not_tls(b"GET"));
    assert!(judged_not_tls(&[0x16, 3, 4]));
    assert!(!judged_not_tls(&[0x16, 3, 1]));
    Ok(())
}

#[tokio::test]
async fn a_header_split_across_writes_is_read_whole_before_it_is_judged() -> Result<(), Box<dyn std::error::Error>> {
    let (mut peer, mut client) = duplex(64);
    let judged = tokio::spawn(async move { first_bytes(&mut client, vec![], Duration::from_secs(5), Refuse).await });
    peer.write_all(&[0x16]).await?;
    tokio::time::sleep(Duration::from_millis(50)).await;
    peer.write_all(&[5, 1]).await?;
    assert_eq!(judged.await?, FirstBytes::NotTls);
    Ok(())
}

#[tokio::test]
async fn already_read_bytes_short_of_a_header_are_read_on_from() -> Result<(), Box<dyn std::error::Error>> {
    let (mut peer, mut client) = duplex(64);
    peer.write_all(&[3, 1]).await?;
    assert_eq!(
        first_bytes(&mut client, vec![0x16], Duration::from_secs(1), Refuse).await,
        FirstBytes::Terminable(vec![0x16, 3, 1])
    );
    Ok(())
}

#[tokio::test]
async fn a_client_that_ends_short_of_a_header_is_left_to_the_handshake() -> Result<(), Box<dyn std::error::Error>> {
    for sent in [&[0x16][..], b"G"] {
        let (mut peer, mut client) = duplex(64);
        peer.write_all(sent).await?;
        drop(peer);
        assert_eq!(
            first_bytes(&mut client, vec![], Duration::from_secs(1), Refuse).await,
            FirstBytes::Terminable(sent.to_vec())
        );
    }
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_silent_client_is_left_to_the_handshake_deadline() -> Result<(), Box<dyn std::error::Error>> {
    let (_peer, mut client) = duplex(64);
    assert_eq!(
        first_bytes(&mut client, vec![], Duration::from_secs(10), Refuse).await,
        FirstBytes::Terminable(vec![])
    );
    Ok(())
}

#[tokio::test]
async fn with_the_switch_off_nothing_is_read() -> Result<(), Box<dyn std::error::Error>> {
    let (mut peer, mut client) = duplex(64);
    peer.write_all(b"SSH-2.0").await?;
    assert_eq!(
        first_bytes(&mut client, vec![], Duration::from_secs(1), Allow).await,
        FirstBytes::Terminable(vec![])
    );
    Ok(())
}

#[test]
fn only_unterminated_allows_are_unread() -> Result<(), Box<dyn std::error::Error>> {
    assert!(would_splice_unread(None, false));
    assert!(would_splice_unread(Some(&DomainRuleAction::PassThrough), false));
    assert!(!would_splice_unread(Some(&DomainRuleAction::PassThrough), true));
    Ok(())
}
