use tokio::io::{AsyncWriteExt, duplex};

use super::*;
use crate::held_writes::{HeldWrites, expect_bytes};

#[test]
fn a_request_without_host_gets_one_after_the_other_fields() -> Result<(), Box<dyn std::error::Error>> {
    let sent = vec![("X-A".to_string(), "1".to_string())];
    assert_eq!(
        framed_headers(&sent, "allowed.test"),
        vec![
            ("x-a".to_string(), "1".to_string()),
            ("host".to_string(), "allowed.test".to_string()),
        ]
    );
    Ok(())
}

#[test]
fn empty_duplicates_are_joined_too() -> Result<(), Box<dyn std::error::Error>> {
    let sent = vec![
        ("X-E".to_string(), String::new()),
        ("x-e".to_string(), "z".to_string()),
        ("x-e".to_string(), String::new()),
    ];
    let first = framed_headers(&sent, "h")
        .into_iter()
        .next()
        .ok_or("expected a header")?;
    assert_eq!(first.1, ", z, ");
    Ok(())
}

#[tokio::test]
async fn a_head_for_a_parent_proxy_is_flushed_before_the_response_is_awaited() -> Result<(), Box<dyn std::error::Error>>
{
    let (parent, mut peer) = duplex(64 * 1024);
    let mut upstream = HeldWrites::new(parent);
    let head = b"GET http://a.test/ HTTP/1.1\r\nHost: a.test\r\n\r\n";
    send_head(&mut upstream, head).await?;
    expect_bytes(&mut peer, head, "the head").await;
    Ok(())
}

#[tokio::test]
async fn a_relayed_body_is_flushed_to_a_parent_proxy_before_the_response_is_awaited()
-> Result<(), Box<dyn std::error::Error>> {
    let (parent, mut peer) = duplex(64 * 1024);
    let mut upstream = HeldWrites::new(parent);
    upstream.write_all(b"hello").await?;
    let relayed = flush_relayed_body(&mut upstream, Ok(5)).await?;
    assert_eq!(relayed, 5);
    expect_bytes(&mut peer, b"hello", "the body").await;
    Ok(())
}

mod serving_tests;
