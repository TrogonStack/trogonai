use super::ByteBudget;
use std::time::Duration;

pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

pub const FIRST_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

pub const IDLE_TIMEOUT: Duration = Duration::from_secs(300);

pub const STALLED_TIMEOUT: Duration = Duration::from_secs(300);

pub const KEEP_ALIVE_TIMEOUT: Duration = Duration::from_secs(6);

pub const GRACE_AFTER_FLUSH: Duration = Duration::from_secs(5);

pub const MAX_QUEUED_FOR_CLIENT: ByteBudget = ByteBudget::from_kib(1024);

pub const SHARED_QUEUE_BUDGET: ByteBudget = ByteBudget::from_bytes(256 << 20);
