use crate::constants::READ_BUFFER_BYTES;
use russh::{ChannelMsg, client};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::TerminalDimension;

/// Moves bytes between one SSH channel and a terminal's pipes until either side ends, then reports the shell's exit
/// status if the sandbox sent one.
pub(crate) async fn pump(
    mut channel: russh::Channel<client::Msg>,
    mut stdin: DuplexStream,
    mut stdout: DuplexStream,
    mut resize: mpsc::Receiver<(TerminalDimension, TerminalDimension)>,
    close: CancellationToken,
    done: oneshot::Sender<i32>,
) {
    let mut buf = vec![0u8; READ_BUFFER_BYTES];
    let mut status = None;
    let mut input_open = true;
    let mut resize_open = true;
    loop {
        tokio::select! {
            _ = close.cancelled() => {
                drop(channel.close().await);
                break;
            }
            message = channel.wait() => match message {
                Some(ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. }) => {
                    if stdout.write_all(&data).await.is_err() {
                        drop(channel.close().await);
                        break;
                    }
                }
                Some(ChannelMsg::ExitStatus { exit_status }) => {
                    status = Some(i32::try_from(exit_status).unwrap_or(i32::MAX));
                }
                Some(ChannelMsg::Close) | None => break,
                Some(_) => {}
            },
            read = stdin.read(&mut buf), if input_open => match read {
                Ok(0) | Err(_) => {
                    input_open = false;
                    drop(channel.eof().await);
                }
                Ok(read) => {
                    // A failed write means the channel is going away; the loop ends when it reports closed.
                    input_open = channel.data(&buf[..read]).await.is_ok();
                }
            },
            size = resize.recv(), if resize_open => match size {
                Some((cols, rows)) => {
                    drop(
                        channel
                            .window_change(u32::from(cols.get()), u32::from(rows.get()), 0, 0)
                            .await,
                    );
                }
                None => resize_open = false,
            },
        }
    }
    drop(stdout);
    if let Some(code) = status {
        let _ = done.send(code);
    }
}
