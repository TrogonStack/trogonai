use std::io;
use tokio::process::{Child, Command};

/// Starts a configured command.
///
/// Implementations return the child exactly as `Command::spawn` produced it.
pub trait SpawnProcess {
    type Error;

    fn spawn(&self, command: &mut Command) -> Result<Child, Self::Error>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TokioProcessSpawner;

impl SpawnProcess for TokioProcessSpawner {
    type Error = io::Error;

    fn spawn(&self, command: &mut Command) -> Result<Child, Self::Error> {
        command.spawn()
    }
}
