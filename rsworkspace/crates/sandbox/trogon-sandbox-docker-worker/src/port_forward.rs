//! Reaching a container port that was not published at create.
//!
//! Docker cannot add a port binding to a running container, so a port that
//! was not in `SandboxSpec::exposed_ports` gets a loopback listener on the
//! worker host instead. Each accepted connection is bridged through a
//! `docker exec` that connects to the port on the container's own loopback.
//! That works under `network_mode: none` too, because the bridge never leaves
//! the container's network namespace.

use bollard::Docker;
use bollard::container::LogOutput;
use bollard::exec::{CreateExecOptions, StartExecOptions, StartExecResults};
use futures::StreamExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tracing::{debug, warn};

mod constants;

pub(crate) use constants::MAX_FORWARDS_PER_LEASE;
use constants::{ACCEPT_RETRY_DELAY, BRIDGE_SCRIPT, RELAY_BUFFER_BYTES};

/// The command a bridge exec runs for `port`.
pub(crate) fn bridge_command(port: u16) -> Vec<String> {
    vec![
        "sh".to_string(),
        "-c".to_string(),
        BRIDGE_SCRIPT.to_string(),
        "sh".to_string(),
        port.to_string(),
    ]
}

/// A loopback listener forwarding to one container port. Dropping it stops
/// accepting; bridges already open end when either side closes.
pub(crate) struct PortForward {
    pub(crate) endpoint: String,
    accept_loop: JoinHandle<()>,
}

impl Drop for PortForward {
    fn drop(&mut self) {
        self.accept_loop.abort();
    }
}

impl PortForward {
    pub(crate) async fn start(docker: Docker, container_id: String, port: u16) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = listener.local_addr()?.to_string();
        let accept_loop = tokio::spawn(async move {
            loop {
                let stream = match listener.accept().await {
                    Ok((stream, _)) => stream,
                    Err(e) => {
                        warn!(error = %e, port, "port forward accept failed");
                        tokio::time::sleep(ACCEPT_RETRY_DELAY).await;
                        continue;
                    }
                };
                let docker = docker.clone();
                let container_id = container_id.clone();
                tokio::spawn(async move {
                    if let Err(e) = bridge(&docker, &container_id, port, stream).await {
                        debug!(error = %e, port, "port forward bridge ended");
                    }
                });
            }
        });
        Ok(Self { endpoint, accept_loop })
    }
}

async fn bridge(docker: &Docker, container_id: &str, port: u16, stream: TcpStream) -> anyhow::Result<()> {
    let exec = docker
        .create_exec(
            container_id,
            CreateExecOptions {
                attach_stdin: Some(true),
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                tty: Some(false),
                cmd: Some(bridge_command(port)),
                ..Default::default()
            },
        )
        .await?;
    let (mut output, mut input) = match docker
        .start_exec(
            &exec.id,
            Some(StartExecOptions {
                detach: false,
                tty: false,
                output_capacity: None,
            }),
        )
        .await?
    {
        StartExecResults::Attached { output, input } => (output, input),
        StartExecResults::Detached => anyhow::bail!("exec detached"),
    };
    let (mut tcp_read, mut tcp_write) = stream.into_split();
    let upstream = async {
        let mut buf = vec![0u8; RELAY_BUFFER_BYTES];
        loop {
            let n = tcp_read.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            input.write_all(&buf[..n]).await?;
            input.flush().await?;
        }
        input.shutdown().await?;
        anyhow::Ok(())
    };
    let downstream = async {
        while let Some(frame) = output.next().await {
            match frame? {
                LogOutput::StdOut { message } => tcp_write.write_all(&message).await?,
                LogOutput::StdErr { message } => {
                    debug!(port, stderr = %String::from_utf8_lossy(&message), "port forward bridge");
                }
                _ => {}
            }
        }
        tcp_write.shutdown().await?;
        anyhow::Ok(())
    };
    // The container side closing ends the connection; the client closing
    // only half-closes, so the reply can still drain.
    tokio::pin!(upstream);
    tokio::pin!(downstream);
    let mut upstream_done = false;
    loop {
        tokio::select! {
            up = &mut upstream, if !upstream_done => {
                upstream_done = true;
                up?;
            }
            down = &mut downstream => return down,
        }
    }
}

#[cfg(test)]
mod tests;
