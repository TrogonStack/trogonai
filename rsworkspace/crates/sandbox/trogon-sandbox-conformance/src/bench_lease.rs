use trogon_sandbox_contract::WriteFilesResponse;

use crate::check_failure::ensure;
use crate::held::Held;
use crate::{CheckFailureError, Target, outcomes};

/// A granted lease for timing the round trips a benchmark measures: seeding a file and reading it back through a
/// command. It is cancelled on [`BenchLease::release`].
pub struct BenchLease<'a>(Held<'a>);

impl<'a> BenchLease<'a> {
    /// Leases from `target` the way every check does.
    pub async fn acquire(target: &'a Target) -> Result<Self, CheckFailureError> {
        Ok(Self(Held::acquire(target).await?))
    }

    /// Writes `len` bytes to `path` in the lease.
    pub async fn seed_file(&self, path: &str, len: usize) -> Result<(), CheckFailureError> {
        let reply = self.0.write(path, &vec![b'x'; len]).await?;
        ensure(
            matches!(reply, WriteFilesResponse::Written),
            format!("{path} to be written"),
            &reply,
        )
    }

    /// Runs `cat path` in the foreground and returns how many bytes it printed.
    pub async fn cat(&self, path: &str) -> Result<usize, CheckFailureError> {
        let out = outcomes::completed(self.0.run(&["cat", path], false).await?)?;
        ensure(out.exit_code == 0, format!("cat {path} to exit 0"), &out.exit_code)?;
        Ok(out.stdout.len())
    }

    /// Cancels the lease.
    pub async fn release(self) {
        self.0.release().await;
    }
}
