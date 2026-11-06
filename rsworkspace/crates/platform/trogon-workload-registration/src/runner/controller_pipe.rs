use std::os::fd::AsFd;
use std::time::Instant;

use tokio::io::AsyncReadExt;
use tokio::net::unix::pipe::Receiver;
use trogon_workload_registration::MAX_SNAPSHOT_BYTES;

use super::runner_error::RunnerError;
use super::signed_frame::SignedFrame;
use super::wait_limit::WaitLimit;

pub(super) struct ControllerPipe(Receiver);

impl ControllerPipe {
    pub fn from_stdin() -> Result<Self, RunnerError> {
        let descriptor = std::io::stdin()
            .as_fd()
            .try_clone_to_owned()
            .map_err(RunnerError::Input)?;
        Receiver::from_owned_fd(descriptor)
            .map(Self)
            .map_err(RunnerError::Input)
    }

    pub async fn next(&mut self, current: Option<Instant>) -> Result<SignedFrame, RunnerError> {
        let idle = match current {
            Some(deadline) => WaitLimit::authority(deadline),
            None => WaitLimit::input(None),
        };
        let mut length = [0u8; 4];
        idle.run(async {
            self.0.read_exact(&mut length[..1]).await.map_err(RunnerError::Input)?;
            Ok(())
        })
        .await?;
        let frame = match current {
            Some(deadline) => WaitLimit::input(Some(deadline)),
            None => idle,
        };
        frame
            .run(async {
                self.0.read_exact(&mut length[1..]).await.map_err(RunnerError::Input)?;
                let length = u32::from_be_bytes(length) as usize;
                if length == 0 || length > MAX_SNAPSHOT_BYTES {
                    return Err(RunnerError::FrameBounds);
                }
                let mut signature = [0u8; 64];
                self.0.read_exact(&mut signature).await.map_err(RunnerError::Input)?;
                let mut payload = vec![0u8; length];
                self.0.read_exact(&mut payload).await.map_err(RunnerError::Input)?;
                Ok(SignedFrame { signature, payload })
            })
            .await
    }
}
