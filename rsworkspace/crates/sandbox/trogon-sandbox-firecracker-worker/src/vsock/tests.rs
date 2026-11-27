use tokio::net::UnixListener;

use super::*;

fn port(value: u32) -> Result<VsockPort, Box<dyn std::error::Error>> {
    Ok(VsockPort::new(value)?)
}

#[tokio::test]
async fn the_handshake_sends_connect_and_leaves_the_stream_raw() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let socket = tmp.path().join("v.sock");
    let listener = UnixListener::bind(&socket)?;

    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await?;
        let mut request = [0u8; 13];
        stream.read_exact(&mut request).await?;
        assert_eq!(&request, b"CONNECT 8110\n");
        stream.write_all(b"OK 1073741824\nhello").await?;
        let mut echoed = [0u8; 4];
        stream.read_exact(&mut echoed).await?;
        assert_eq!(&echoed, b"ping");
        Ok::<(), std::io::Error>(())
    });

    let mut stream = VsockEndpoint::new(socket, port(8110)?).connect().await?;
    let mut payload = [0u8; 5];
    stream.read_exact(&mut payload).await?;
    assert_eq!(&payload, b"hello", "bytes after the OK line must not be consumed");
    stream.write_all(b"ping").await?;
    server.await??;
    Ok(())
}

#[tokio::test]
async fn a_closed_connection_means_the_guest_is_not_listening() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let socket = tmp.path().join("v.sock");
    let listener = UnixListener::bind(&socket)?;
    tokio::spawn(async move {
        if let Ok((mut stream, _)) = listener.accept().await {
            let mut buf = [0u8; 32];
            let _ = stream.read(&mut buf).await;
        }
    });

    let err = VsockEndpoint::new(socket, port(8110)?)
        .connect()
        .await
        .err()
        .ok_or("expected an error")?;
    assert!(matches!(err, VsockError::Handshake { .. }), "{err}");
    Ok(())
}

#[tokio::test]
async fn an_unexpected_reply_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let socket = tmp.path().join("v.sock");
    let listener = UnixListener::bind(&socket)?;
    tokio::spawn(async move {
        if let Ok((mut stream, _)) = listener.accept().await {
            let mut buf = [0u8; 32];
            let _ = stream.read(&mut buf).await;
            let _ = stream.write_all(b"NOPE\n").await;
        }
    });

    let err = VsockEndpoint::new(socket, port(8110)?)
        .connect()
        .await
        .err()
        .ok_or("expected an error")?;
    assert!(format!("{err:?}").contains("NOPE"), "{err}");
    Ok(())
}

#[tokio::test]
async fn a_missing_socket_is_a_connect_error() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let err = VsockEndpoint::new(tmp.path().join("none.sock"), port(1)?)
        .connect()
        .await
        .err()
        .ok_or("expected an error")?;
    assert!(matches!(err, VsockError::Connect { .. }));
    Ok(())
}
