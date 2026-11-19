use std::time::Duration;

use trogon_std::{ByteSize, HttpBodySizeMax};

pub const HTTP_BODY_SIZE_MAX: HttpBodySizeMax = HttpBodySizeMax::new(ByteSize::b(3_000_000)).unwrap();
pub(super) const MAX_BATCH_UPDATES: usize = 1_000;
pub(super) const DEDUP_WINDOW: Duration = Duration::from_secs(7 * 24 * 60 * 60);
pub(super) const PUBLISH_CONCURRENCY: usize = 16;
