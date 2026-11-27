use chrono::{Datelike, NaiveDate, Utc};
use trogon_sandbox_broker::quota::{
    NatsKvQuotaStore, QuotaRecord, legacy_quota_kv_key_for_month, quota_kv_key_for_month,
};
use trogon_sandbox_contract::TenantId;

use crate::{AdminError, Console};

/// NATS KV keys of a tenant's quota record for one month: the current key the broker writes,
/// then the legacy key it seeds the current key from.
pub fn quota_kv_keys(tenant: &TenantId, year: i32, month: u32) -> (String, String) {
    (
        quota_kv_key_for_month(tenant, year, month),
        legacy_quota_kv_key_for_month(tenant, year, month),
    )
}

/// Parses `YYYY-MM`.
pub fn parse_month(raw: &str) -> Result<(i32, u32), AdminError> {
    let date = NaiveDate::parse_from_str(&format!("{raw}-01"), "%Y-%m-%d").map_err(AdminError::InvalidMonth)?;
    Ok((date.year(), date.month()))
}

/// Shows a tenant's usage for `month`, or for the current month.
pub async fn show(
    store: &NatsKvQuotaStore,
    tenant: &TenantId,
    month: Option<(i32, u32)>,
    console: &mut Console<'_>,
) -> Result<(), AdminError> {
    let (year, mon) = month.unwrap_or_else(|| {
        let now = Utc::now();
        (now.year(), now.month())
    });
    // Until the broker first touches the month under the current key, the record only exists
    // under the legacy key.
    let (key, legacy) = quota_kv_keys(tenant, year, mon);
    let read = AdminError::step("read quota record");
    let bytes = match store.kv.get(&key).await.map_err(read)? {
        Some(bytes) => Some(bytes),
        None => store
            .kv
            .get(&legacy)
            .await
            .map_err(AdminError::step("read quota record"))?,
    };
    let Some(bytes) = bytes else {
        return console.out(format_args!(
            "no quota record for tenant '{tenant}' month {year:04}-{mon:02}"
        ));
    };
    let record: QuotaRecord = serde_json::from_slice(&bytes).map_err(|source| AdminError::Parse {
        what: "quota record",
        source,
    })?;
    console.out(format_args!(
        "cpu:  {} / {} seconds used",
        record.cpu_seconds_used, record.cap_cpu_seconds
    ))?;
    console.out(format_args!(
        "wall: {} / {} seconds used",
        record.wall_seconds_used, record.cap_wall_seconds
    ))
}
