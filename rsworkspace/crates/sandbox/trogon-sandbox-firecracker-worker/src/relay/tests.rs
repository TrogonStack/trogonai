use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;

fn os(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

#[test]
fn arguments_after_the_separator_belong_to_the_program() -> Result<(), Box<dyn std::error::Error>> {
    let parsed = RelayArgs::parse(os(&[
        "--listen",
        "172.16.0.1:3128",
        "--socket",
        "/s/proxy.sock",
        "--pid-file",
        "/s/firecracker.pid",
        "--",
        "/bin/firecracker",
        "--api-sock",
        "/s/api.sock",
    ]))?;
    assert_eq!(parsed.listen.to_string(), "172.16.0.1:3128");
    assert_eq!(parsed.program, "/bin/firecracker");
    assert_eq!(parsed.program_args, os(&["--api-sock", "/s/api.sock"]));
    Ok(())
}

#[test]
fn missing_or_unknown_flags_are_refused() {
    assert!(RelayArgs::parse(os(&["--", "x"])).is_err());
    assert!(RelayArgs::parse(os(&["--bogus", "1", "--", "x"])).is_err());
    assert!(RelayArgs::parse(os(&["--listen", "nope", "--socket", "s", "--pid-file", "p", "--", "x"])).is_err());
}

#[tokio::test]
async fn connections_reach_the_socket_and_the_relay_exits_with_its_program() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let socket = tmp.path().join("proxy.sock");
    let upstream = tokio::net::UnixListener::bind(&socket)?;
    tokio::spawn(async move {
        if let Ok((mut stream, _)) = upstream.accept().await {
            let mut buf = [0u8; 4];
            if stream.read_exact(&mut buf).await.is_ok() {
                let _ = stream.write_all(&buf).await;
            }
        }
    });

    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let listen = listener.local_addr()?;
    let args = RelayArgs {
        listen,
        socket,
        pid_file: tmp.path().join("relay.pid"),
        program: "sh".into(),
        program_args: os(&["-c", "sleep 1; exit 7"]),
    };
    let pid_file = args.pid_file.clone();
    let relay = tokio::spawn(relay(listener, args));

    let mut client = tokio::net::TcpStream::connect(listen).await?;
    client.write_all(b"ping").await?;
    let mut echoed = [0u8; 4];
    client.read_exact(&mut echoed).await?;
    assert_eq!(&echoed, b"ping");
    assert_eq!(std::fs::read_to_string(pid_file)?, std::process::id().to_string());
    assert_eq!(relay.await??, 7);
    Ok(())
}
