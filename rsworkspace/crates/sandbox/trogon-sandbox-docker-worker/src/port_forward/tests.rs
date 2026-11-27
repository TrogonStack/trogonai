use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::*;
use crate::support_tests::TestResult;
#[cfg(unix)]
use std::os::unix::fs::symlink;
use std::process::Stdio;

#[test]
fn bridge_command_passes_the_port_as_the_first_script_argument() -> TestResult {
    let cmd = bridge_command(8080);
    assert_eq!(cmd.get(..2), Some(&["sh".to_string(), "-c".to_string()][..]));
    assert_eq!(cmd.get(3).map(String::as_str), Some("sh"));
    assert_eq!(cmd.get(4).map(String::as_str), Some("8080"));
    let script = cmd.get(2).ok_or("no script")?;
    assert!(script.contains("socat"));
    assert!(script.contains("/dev/tcp/127.0.0.1/$0"));
    assert!(script.contains("remote_closed"));
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn bash_only_bridge_delivers_the_clients_http_request() -> TestResult {
    let tools = tempfile::tempdir()?;
    symlink("/bin/bash", tools.path().join("bash"))?;
    symlink("/bin/cat", tools.path().join("cat"))?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let server = tokio::spawn(async move {
        let (mut connection, _) = listener.accept().await?;
        let request = tokio::time::timeout(Duration::from_secs(5), async {
            let mut bytes = Vec::new();
            while !bytes.ends_with(b"\r\n\r\n") {
                let byte = connection.read_u8().await?;
                bytes.push(byte);
            }
            std::io::Result::Ok(bytes)
        })
        .await??;
        connection
            .write_all(b"HTTP/1.0 200 OK\r\nContent-Length: 2\r\n\r\nok")
            .await?;
        connection.shutdown().await?;
        std::io::Result::Ok(request)
    });
    let mut command = tokio::process::Command::new("/bin/sh")
        .args(bridge_command(port).into_iter().skip(1))
        .env("PATH", tools.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let mut input = command.stdin.take().ok_or("no stdin")?;
    input.write_all(b"GET / HTTP/1.0\r\nHost: local\r\n\r\n").await?;
    input.shutdown().await?;
    drop(input);
    let request = server.await??;
    let output = tokio::time::timeout(Duration::from_secs(5), command.wait_with_output()).await??;
    assert_eq!(request, b"GET / HTTP/1.0\r\nHost: local\r\n\r\n");
    assert!(output.status.success());
    assert!(output.stdout.ends_with(b"\r\n\r\nok"));
    Ok(())
}
