use std::time::Duration;

/// Longest caller- or peer-chosen string quoted back into model-facing text.
pub(crate) const MAX_QUOTED_CHARS: usize = 80;

/// Most peers named in one piece of recipient guidance.
pub(crate) const MAX_LISTED_PEERS: usize = 8;

/// Largest roster page the connector reads on its own to word guidance.
pub(crate) const DIRECTORY_PAGE_SIZE: u32 = 64;

/// Longest the connector waits on a roster read it makes only to word guidance.
pub(crate) const DIRECTORY_BUDGET: Duration = Duration::from_millis(500);

/// Time kept back from a call's deadline when a guidance roster read runs inside it.
pub(crate) const DIRECTORY_MARGIN: Duration = Duration::from_millis(100);
