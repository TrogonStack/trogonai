use std::fmt::Display;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};

use tokio::sync::mpsc;

use crate::args::ControlFd;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix::open_reader;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows::open_reader;
mod constants;
#[cfg(test)]
mod tests;
use constants::BACKLOG;

/// What arrives on the control channel.
#[derive(Debug)]
pub(crate) enum ControlEvent {
    Line(String),
    Closed,
    Failed(io::Error),
}

/// The read end of the channel, checked and set apart from the command. On
/// Unix the descriptor number the caller gave now holds `/dev/null` for the
/// command to inherit; on Windows the handle behind it is no longer
/// inheritable. Only this process keeps the real channel.
pub(crate) struct ControlChannel {
    fd: ControlFd,
    reader: File,
}

impl ControlChannel {
    pub(crate) fn open(fd: ControlFd) -> Result<Self, String> {
        open_reader(fd).map(|reader| Self { fd, reader })
    }

    pub(crate) fn fd(&self) -> ControlFd {
        self.fd
    }

    /// Reads lines on a thread of its own: a descriptor the caller keeps open
    /// never ends, and must not keep this process from exiting.
    pub(crate) fn listen(self) -> mpsc::Receiver<ControlEvent> {
        let (sender, receiver) = mpsc::channel(BACKLOG);
        std::thread::spawn(move || {
            let mut reader = BufReader::new(self.reader);
            loop {
                let mut line = Vec::new();
                let event = match reader.read_until(b'\n', &mut line) {
                    Ok(0) => ControlEvent::Closed,
                    Ok(_) => ControlEvent::Line(String::from_utf8_lossy(&line).into_owned()),
                    Err(err) => ControlEvent::Failed(err),
                };
                let last = !matches!(event, ControlEvent::Line(_));
                if sender.blocking_send(event).is_err() || last {
                    break;
                }
            }
        });
        receiver
    }
}

pub(crate) async fn next_event(updates: &mut Option<mpsc::Receiver<ControlEvent>>) -> ControlEvent {
    match updates {
        Some(receiver) => receiver.recv().await.unwrap_or(ControlEvent::Closed),
        None => std::future::pending().await,
    }
}

/// Writes one line to standard error.
pub(crate) fn report(message: impl Display) {
    // A closed standard error leaves nowhere to report the failure to.
    let _ = writeln!(io::stderr().lock(), "{message}");
}
