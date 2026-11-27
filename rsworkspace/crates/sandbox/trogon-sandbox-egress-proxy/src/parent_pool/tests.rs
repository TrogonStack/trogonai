use super::*;

#[test]
fn a_hint_shortens_the_free_time_by_a_second_and_never_lengthens_it() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(KeepAliveHint::none().free_for(), Some(FREE_TIMEOUT));
    assert_eq!(
        KeepAliveHint::parse("timeout=2").free_for(),
        Some(Duration::from_secs(1))
    );
    assert_eq!(
        KeepAliveHint::parse("timeout=10, max=100").free_for(),
        Some(FREE_TIMEOUT)
    );
    assert_eq!(KeepAliveHint::parse("timeout=1").free_for(), None);
    assert_eq!(KeepAliveHint::parse("timeout=0").free_for(), None);
    Ok(())
}

#[test]
fn a_hint_is_read_only_from_a_leading_timeout() -> Result<(), Box<dyn std::error::Error>> {
    for value in ["max=5, timeout=2", "Timeout=2", "timeout=", "timeout=x"] {
        assert_eq!(KeepAliveHint::parse(value).free_for(), Some(FREE_TIMEOUT), "{value:?}");
    }
    assert_eq!(
        KeepAliveHint::parse("timeout=3x").free_for(),
        Some(Duration::from_secs(2))
    );
    Ok(())
}

#[tokio::test]
async fn at_most_256_free_connections_are_kept_per_parent() -> Result<(), Box<dyn std::error::Error>> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let key = PoolKey {
        secured: false,
        host: "parent.test".to_owned(),
        port: addr.port(),
    };
    let pool = ParentPool::default();
    let mut accepted = Vec::new();
    for _ in 0..=MAX_FREE {
        let stream = tokio::net::TcpStream::connect(addr).await?;
        accepted.push(listener.accept().await?.0);
        pool.give_back(key.clone(), stream.into(), KeepAliveHint::none());
    }
    assert_eq!(pool.lock().free[&key].len(), MAX_FREE);
    Ok(())
}

async fn connected_pair(
    listener: &tokio::net::TcpListener,
) -> Result<(tokio::net::TcpStream, tokio::net::TcpStream), Box<dyn std::error::Error>> {
    let client = tokio::net::TcpStream::connect(listener.local_addr()?).await?;
    let (server, _) = listener.accept().await?;
    Ok((client, server))
}

fn test_key(port: u16) -> PoolKey {
    PoolKey {
        secured: false,
        host: "parent.test".to_owned(),
        port,
    }
}

#[tokio::test]
async fn a_closed_free_connection_is_skipped_and_a_live_one_is_handed_out() -> Result<(), Box<dyn std::error::Error>> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let key = test_key(listener.local_addr()?.port());
    let pool = ParentPool::default();
    let (live, _live_peer) = connected_pair(&listener).await?;
    let (dead, dead_peer) = connected_pair(&listener).await?;
    drop(dead_peer);
    dead.readable().await?;
    pool.give_back(key.clone(), live.into(), KeepAliveHint::none());
    pool.give_back(key.clone(), dead.into(), KeepAliveHint::none());
    assert!(pool.take(&key).is_some());
    assert!(pool.take(&key).is_none());
    Ok(())
}

#[tokio::test]
async fn a_pool_with_only_closed_connections_hands_out_none() -> Result<(), Box<dyn std::error::Error>> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let key = test_key(listener.local_addr()?.port());
    let pool = ParentPool::default();
    let (dead, dead_peer) = connected_pair(&listener).await?;
    drop(dead_peer);
    dead.readable().await?;
    pool.give_back(key.clone(), dead.into(), KeepAliveHint::none());
    assert!(pool.take(&key).is_none());
    Ok(())
}

#[test]
fn expiring_after_the_pool_is_gone_does_nothing() {
    expire(&Weak::new(), &test_key(1), 1);
}

#[tokio::test]
async fn expiring_one_connection_keeps_the_others_free() -> Result<(), Box<dyn std::error::Error>> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let key = test_key(listener.local_addr()?.port());
    let pool = ParentPool::default();
    let (first, _first_peer) = connected_pair(&listener).await?;
    let (second, _second_peer) = connected_pair(&listener).await?;
    pool.give_back(key.clone(), first.into(), KeepAliveHint::none());
    pool.give_back(key.clone(), second.into(), KeepAliveHint::none());
    expire(&Arc::downgrade(&pool.pools), &key, 1);
    assert_eq!(pool.lock().free[&key].len(), 1);
    Ok(())
}
