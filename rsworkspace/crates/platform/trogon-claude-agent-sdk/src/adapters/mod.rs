mod environment;
mod extract;
mod filesystem;
mod host;
mod process;
mod wall_clock;

pub use environment::{CurrentDirectory, EnvironmentVariable, SystemEnvironment, TemporaryDirectory};
pub use extract::*;
pub use filesystem::*;
pub use host::*;
pub use process::{SpawnProcess, TokioProcessSpawner};
pub use wall_clock::{SystemClock, WallClock};

#[derive(Clone, Debug, Default)]
pub struct RuntimeAdapters<E = SystemEnvironment, P = TokioProcessSpawner, F = NativeFileSystem> {
    pub environment: E,
    pub process: P,
    pub filesystem: F,
}
