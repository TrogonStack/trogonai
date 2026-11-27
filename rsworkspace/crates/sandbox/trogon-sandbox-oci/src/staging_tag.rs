use trogon_sandbox_contract::Digest;

use crate::constants::STAGING_TAG_PREFIX;

/// Digest an artifact staging tag was derived from, or `None` for any other tag.
pub fn digest_from_staging_tag(tag: &str) -> Option<Digest> {
    let hex = tag.strip_prefix(STAGING_TAG_PREFIX)?.strip_prefix("sha256-")?;
    Digest::new(format!("sha256:{hex}")).ok()
}
