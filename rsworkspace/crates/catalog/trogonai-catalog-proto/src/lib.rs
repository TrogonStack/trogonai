#![allow(clippy::all)]

mod proto {
    include!(concat!(env!("OUT_DIR"), "/_include.rs"));
}

pub use proto::trogonai::catalog::v1::*;
