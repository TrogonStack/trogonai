#![allow(clippy::all)]

pub mod trogon {
    pub mod cron {
        pub mod jobs {
            pub mod v1 {
                include!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/src/gen/trogon/cron/jobs/v1/generated.rs"
                ));
            }

            pub mod state {
                pub mod v1 {
                    include!(concat!(
                        env!("CARGO_MANIFEST_DIR"),
                        "/src/gen/trogon/cron/jobs/state/v1/generated.rs"
                    ));
                }
            }
        }
    }
}

pub use trogon::cron::jobs::state::v1 as state_v1;
pub use trogon::cron::jobs::v1;
