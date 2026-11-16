use futures::future::{BoxFuture, Shared};

use super::{ReleaseError, ReleasedValue};

/// What a call to the secrets service landed with: the release, nothing when
/// it was overtaken on its way, or why it failed.
pub(crate) type Landed = Result<Option<ReleasedValue>, ReleaseError>;

/// One call to the secrets service on its way for a cache key, which every
/// miss for that key waits on instead of asking again.
///
/// The call runs as a task of its own, so it lands whether or not anyone is
/// still waiting on it. The ticket names the epoch it was asked in, so a miss
/// can tell a call an invalidation already overtook from one it may join.
#[derive(Clone)]
pub(crate) struct ReleaseFlight {
    id: u64,
    ticket: u64,
    landed: Shared<BoxFuture<'static, Landed>>,
}

impl ReleaseFlight {
    pub(crate) const fn new(id: u64, ticket: u64, landed: Shared<BoxFuture<'static, Landed>>) -> Self {
        Self { id, ticket, landed }
    }

    pub(crate) const fn id(&self) -> u64 {
        self.id
    }

    pub(crate) const fn ticket(&self) -> u64 {
        self.ticket
    }

    /// Waits for the call to land; dropping the wait leaves the call going.
    pub(crate) async fn landed(self) -> Landed {
        self.landed.await
    }
}
