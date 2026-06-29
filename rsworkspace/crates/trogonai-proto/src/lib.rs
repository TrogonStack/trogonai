#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

#[allow(clippy::all)]
#[cfg(feature = "schedules")]
mod r#gen;

#[cfg(feature = "schedules")]
mod codec;

#[cfg(feature = "chrono")]
pub mod convert;

#[cfg(feature = "schedules")]
pub mod scheduler;

// Thin wrappers that re-export the generated proto packages, emitted as inline
// module trees that mirror the codegen layout.
#[cfg(feature = "schedules")]
#[cfg_attr(dylint_lib = "trogon_lints", allow(inline_module_block))]
pub mod content {
    pub mod v1alpha1 {
        pub use crate::r#gen::trogon::content::v1alpha1::*;
    }
}

#[cfg(feature = "schedules")]
#[cfg_attr(dylint_lib = "trogon_lints", allow(inline_module_block))]
pub mod google {
    pub mod r#type {
        pub use crate::r#gen::google::r#type::*;
    }
}
