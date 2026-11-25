use std::time::Duration;

pub const WAIT: Duration = Duration::from_secs(8);
pub const TICK: Duration = Duration::from_millis(25);
pub const LOCAL_WORKLOAD: &str = "spiffe://trogon.example/service/egress";
pub const PEER_WORKLOAD: &str = "spiffe://trogon.example/runtime/opaque-selector";
pub const APPLICATION_ID: &str = "did:key:z6MkhaXgBZDvotDkL5257faiztiGi2sfKfzvuU5TVndZqquw";
