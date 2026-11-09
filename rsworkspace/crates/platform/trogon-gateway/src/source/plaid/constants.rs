use std::time::Duration;

use trogon_std::{ByteSize, HttpBodySizeMax};

pub const HTTP_BODY_SIZE_MAX: HttpBodySizeMax = HttpBodySizeMax::new(ByteSize::mib(2)).unwrap();

pub const HEADER_VERIFICATION: &str = "plaid-verification";

pub const NATS_HEADER_EVENT_TYPE: &str = "X-Plaid-Event-Type";
pub const NATS_HEADER_ITEM_ID: &str = "X-Plaid-Item-Id";
pub const NATS_HEADER_REJECT_REASON: &str = "X-Plaid-Reject-Reason";

pub const SANDBOX_API_BASE_URL: &str = "https://sandbox.plaid.com";
pub const PRODUCTION_API_BASE_URL: &str = "https://production.plaid.com";

pub const WEBHOOK_VERIFICATION_KEY_PATH: &str = "webhook_verification_key/get";
pub const KEY_FETCH_TIMEOUT: Duration = Duration::from_secs(5);
pub const KEY_REFRESH_INTERVAL: Duration = Duration::from_secs(60 * 60);
pub const KEY_FETCH_THROTTLE_INTERVAL: Duration = Duration::from_secs(10);
pub const UNKNOWN_KEY_ERROR_CODE: &str = "INVALID_WEBHOOK_VERIFICATION_KEY_ID";

pub const KEY_ID_LEN_MAX: usize = 128;
pub const ITEM_ID_LEN_MAX: usize = 128;
