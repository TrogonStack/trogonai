pub const PROTOCOL: u64 = 1;

pub const MAX_READ_FRAME: usize = 1 << 20;

pub const MAX_BODY_BYTES_HELD: usize = 64 << 20;

pub(super) const QUEUED_REQUEST_FRAME_ROOM: usize = 32 << 20;

pub(super) const MAX_ID: u64 = (1 << 53) - 1;

pub(super) const ID_WINDOW: u64 = 4096;

pub(super) const BODYLESS_METHOD_BODY_REASON: &str =
    "the decider asked to see the body of a request whose method normally has none, and it has one";
