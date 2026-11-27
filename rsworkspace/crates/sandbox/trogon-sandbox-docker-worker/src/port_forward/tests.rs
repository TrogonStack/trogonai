use std::time::Duration;

use anyhow::{Context, ensure};
use bollard::models::{ContainerCreateBody, HostConfig};
use bollard::query_parameters::{CreateContainerOptionsBuilder, RemoveContainerOptionsBuilder, StartContainerOptions};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::*;
use crate::fresh_id::fresh_id;
use crate::support_tests::TestResult;
#[cfg(unix)]
use std::os::unix::fs::symlink;
use std::process::Stdio;

/// The image the forward lanes run in. Its busybox `nc` listens with `-e`, and it has no socat or bash, so the bridge
/// falls back to `nc`.
const IMAGE: &str = "alpine:3.20";
const BOUND: Duration = Duration::from_secs(15);

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

/// A container running `sleep` on the local daemon, removed by [`Container::remove`].
struct Container {
    docker: Docker,
    id: String,
}

impl Container {
    async fn run(network: &str) -> anyhow::Result<Self> {
        let docker = Docker::connect_with_local_defaults()?;
        docker.info().await?;
        let created = docker
            .create_container(
                Some(
                    CreateContainerOptionsBuilder::new()
                        .name(&format!("port-forward-{}", fresh_id()))
                        .build(),
                ),
                ContainerCreateBody {
                    image: Some(IMAGE.into()),
                    cmd: Some(vec!["sleep".into(), "120".into()]),
                    host_config: Some(HostConfig {
                        network_mode: Some(network.into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )
            .await?;
        let container = Self { docker, id: created.id };
        if let Err(error) = container
            .docker
            .start_container(&container.id, None::<StartContainerOptions>)
            .await
        {
            container.remove().await?;
            return Err(error.into());
        }
        Ok(container)
    }

    /// Runs `script` to completion and returns its exit code.
    async fn exec(&self, script: &str) -> anyhow::Result<i64> {
        let exec = self
            .docker
            .create_exec(
                &self.id,
                CreateExecOptions::<String> {
                    cmd: Some(vec!["sh".into(), "-c".into(), script.into()]),
                    attach_stdout: Some(true),
                    attach_stderr: Some(true),
                    ..Default::default()
                },
            )
            .await?;
        if let StartExecResults::Attached { mut output, .. } = self.docker.start_exec(&exec.id, None).await? {
            while let Some(chunk) = output.next().await {
                chunk?;
            }
        }
        self.docker
            .inspect_exec(&exec.id)
            .await?
            .exit_code
            .context("exec finished without an exit code")
    }

    /// Starts `script` in the background.
    async fn exec_detached(&self, script: &str) -> anyhow::Result<()> {
        let exec = self
            .docker
            .create_exec(
                &self.id,
                CreateExecOptions::<String> {
                    cmd: Some(vec!["sh".into(), "-c".into(), script.into()]),
                    ..Default::default()
                },
            )
            .await?;
        self.docker
            .start_exec(
                &exec.id,
                Some(StartExecOptions {
                    detach: true,
                    ..Default::default()
                }),
            )
            .await?;
        Ok(())
    }

    async fn forward_to(&self, port: u16) -> anyhow::Result<PortForward> {
        Ok(PortForward::start(self.docker.clone(), self.id.clone(), port).await?)
    }

    async fn remove(self) -> anyhow::Result<()> {
        self.docker
            .remove_container(&self.id, Some(RemoveContainerOptionsBuilder::new().force(true).build()))
            .await?;
        Ok(())
    }
}

/// Reads the whole reply on a forwarded connection, failing when it does not end within the bound.
async fn reply(stream: &mut TcpStream, hang: &str) -> anyhow::Result<String> {
    let mut reply = Vec::new();
    tokio::time::timeout(BOUND, stream.read_to_end(&mut reply))
        .await
        .context(hang.to_owned())??;
    Ok(String::from_utf8_lossy(&reply).trim().to_owned())
}

/// Writes a line and the client's end of input, then reads the whole reply.
async fn half_closed_reply(forward: &PortForward) -> anyhow::Result<String> {
    let mut stream = TcpStream::connect(&forward.endpoint).await?;
    stream.write_all(b"ping\n").await?;
    stream.shutdown().await?;
    reply(&mut stream, "the forward never finished the connection").await
}

/// The server answers only after it sees the client's end of input, and does so slowly: the forward has to carry the
/// client's half-close in and keep the reply flowing out afterwards.
#[tokio::test]
async fn a_client_half_close_reaches_the_server_and_the_reply_still_drains() -> anyhow::Result<()> {
    let container = Container::run("none").await?;
    let observed = async {
        container
            .exec_detached(
                "nc -l -p 9000 -e sh -c 'IFS= read -r line; cat >/dev/null; sleep 1; echo \"eof-seen:$line\"'",
            )
            .await?;
        tokio::time::sleep(Duration::from_secs(1)).await;
        let forward = container.forward_to(9000).await?;
        half_closed_reply(&forward).await
    }
    .await;
    container.remove().await?;
    assert_eq!(observed?, "eof-seen:ping");
    Ok(())
}

/// The server closes first while the client keeps its side open: the client must see the end of the reply rather than
/// hang.
#[tokio::test]
async fn the_container_closing_first_ends_the_client_connection() -> anyhow::Result<()> {
    let container = Container::run("none").await?;
    let observed = async {
        container.exec_detached("nc -l -p 9001 -e sh -c 'echo bye'").await?;
        tokio::time::sleep(Duration::from_secs(1)).await;
        let forward = container.forward_to(9001).await?;
        let mut stream = TcpStream::connect(&forward.endpoint).await?;
        reply(&mut stream, "the client was left hanging after the server closed").await
    }
    .await;
    container.remove().await?;
    assert_eq!(observed?, "bye");
    Ok(())
}

/// The same two directions through socat, which the bridge prefers when the image has it.
#[tokio::test]
#[ignore = "requires Docker and network access to install socat with apk; run with --ignored"]
async fn socat_bridges_both_half_closes_too() -> anyhow::Result<()> {
    let container = Container::run("bridge").await?;
    let observed = async {
        let installed = container.exec("apk add -q --no-cache socat").await?;
        ensure!(installed == 0, "could not install socat: apk exited with {installed}");
        container.exec_detached("nc -l -p 9002 -e sh -c 'echo bye'").await?;
        container
            .exec_detached(
                "nc -l -p 9003 -e sh -c 'IFS= read -r line; cat >/dev/null; sleep 1; echo \"eof-seen:$line\"'",
            )
            .await?;
        tokio::time::sleep(Duration::from_secs(1)).await;
        let closes_first = container.forward_to(9002).await?;
        let mut stream = TcpStream::connect(&closes_first.endpoint).await?;
        let closed = reply(&mut stream, "the client was left hanging after the server closed").await?;
        let half_closes = container.forward_to(9003).await?;
        let half_closed = half_closed_reply(&half_closes).await?;
        Ok::<_, anyhow::Error>((closed, half_closed))
    }
    .await;
    container.remove().await?;
    let (closed, half_closed) = observed?;
    assert_eq!(closed, "bye");
    assert_eq!(half_closed, "eof-seen:ping");
    Ok(())
}
