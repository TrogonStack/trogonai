#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]
// Generated protobuf code (`gen/*`) and the thin wrappers over it are emitted
// as inline module trees by the proto codegen; the policy does not apply here.
#![cfg_attr(dylint_lib = "trogon_lints", allow(inline_module_block))]

#[allow(clippy::all)]
#[path = "gen/mod.rs"]
#[cfg(feature = "schedules")]
mod r#gen;

#[cfg(feature = "schedules")]
mod codec;

#[cfg(feature = "chrono")]
pub mod convert;

#[cfg(feature = "schedules")]
pub mod scheduler;

#[cfg(feature = "schedules")]
pub mod content {
    pub mod v1alpha1 {
        pub use crate::r#gen::trogon::content::v1alpha1::*;
    }
}

#[cfg(feature = "schedules")]
pub mod google {
    pub mod r#type {
        pub use crate::r#gen::google::r#type::*;
    }
}
