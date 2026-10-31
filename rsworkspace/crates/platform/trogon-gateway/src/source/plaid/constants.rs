use std::time::Duration;

pub const SANDBOX_API_BASE_URL: &str = "https://sandbox.plaid.com";
pub const PRODUCTION_API_BASE_URL: &str = "https://production.plaid.com";

pub const WEBHOOK_VERIFICATION_KEY_PATH: &str = "webhook_verification_key/get";
pub const KEY_FETCH_TIMEOUT: Duration = Duration::from_secs(5);

pub const KEY_ID_LEN_MAX: usize = 128;
