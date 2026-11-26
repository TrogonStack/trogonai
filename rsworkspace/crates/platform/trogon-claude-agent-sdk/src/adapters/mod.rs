mod environment;
mod extract;
mod filesystem;
mod get_addr_info_resolver;
mod host;
mod http;
mod passwd;
mod process;
mod wall_clock;

pub use environment::{
    CurrentDirectory, EnvironmentVariable, SystemEnvironment, TemporaryDirectory, capture_launch_directory,
};
pub use extract::*;
pub use filesystem::*;
pub use get_addr_info_resolver::{GetAddrInfoResolver, NameLookupError};
pub use host::*;
pub use http::HttpExecute;
pub use process::{SpawnProcess, TokioProcessSpawner};
pub use wall_clock::{SystemClock, WallClock};

#[derive(Clone, Debug, Default)]
pub struct RuntimeAdapters<E = SystemEnvironment, P = TokioProcessSpawner, F = NativeFileSystem> {
    pub environment: E,
    pub process: P,
    pub filesystem: F,
}
