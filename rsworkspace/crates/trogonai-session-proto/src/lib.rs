#![allow(clippy::all)]

mod proto {
    include!(concat!(env!("OUT_DIR"), "/_include.rs"));
}

pub use proto::trogonai::session::v1::*;
// `SessionRecord.messages` embeds the compactor `Message` (imported proto), which
// buffa regenerates inside this crate. Re-export it so consumers can build it.
pub use proto::trogonai::compactor::v1 as compactor;
