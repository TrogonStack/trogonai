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
