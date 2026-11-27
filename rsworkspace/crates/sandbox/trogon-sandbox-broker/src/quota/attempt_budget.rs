use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{Datelike, Utc};
use trogon_sandbox_contract::{AttemptQuotaReservation, LeaseAccounting, LeaseError};

use super::attempt_quota_limits::AttemptQuotaLimits;
use super::attempt_quota_receipt::AttemptQuotaPhase;
use super::quota_record::QuotaRecord;
use super::quota_store_error::QuotaStoreError;
use crate::constants::{ATTEMPT_CLOCK_SKEW_TOLERANCE, MAX_LEASE_TTL_SECONDS, QUOTA_RETENTION};

pub(super) fn attempt_error(message: impl Into<String>) -> LeaseError {
    LeaseError::Backend {
        message: message.into(),
    }
}

pub(super) fn attempt_month(reservation: &AttemptQuotaReservation) -> Result<(i32, u32), LeaseError> {
    month_of(reservation.created_at)
}

/// The UTC calendar month a quota record covering `at` is keyed under.
pub(super) fn month_of(at: SystemTime) -> Result<(i32, u32), LeaseError> {
    let time = at
        .duration_since(UNIX_EPOCH)
        .map_err(|_| attempt_error("quota creation time precedes Unix epoch"))?;
    let seconds: i64 = time
        .as_secs()
        .try_into()
        .map_err(|_| attempt_error("quota creation time is invalid"))?;
    let date = chrono::DateTime::<Utc>::from_timestamp(seconds, time.subsec_nanos())
        .ok_or_else(|| attempt_error("quota creation time is invalid"))?;
    Ok((date.year(), date.month()))
}

pub(super) fn attempt_month_key(reservation: &AttemptQuotaReservation) -> Result<String, LeaseError> {
    let (year, month) = attempt_month(reservation)?;
    Ok(format!("{}:{year:04}-{month:02}", reservation.tenant.as_str()))
}

pub(super) fn new_attempt_record(limits: &AttemptQuotaLimits) -> QuotaRecord {
    let mut record = QuotaRecord::new(limits.cpu, limits.wall);
    record.active_lease_cap = limits.active;
    record
}

fn unseen_attempt_age(reservation: &AttemptQuotaReservation) -> Result<Duration, LeaseError> {
    match SystemTime::now().duration_since(reservation.created_at) {
        Ok(age) => Ok(age),
        Err(error) if error.duration() <= ATTEMPT_CLOCK_SKEW_TOLERANCE => Ok(Duration::ZERO),
        Err(_) => Err(attempt_error("quota attempt creation time is in the future")),
    }
}

pub(super) fn validate_unseen_attempt(
    reservation: &AttemptQuotaReservation,
    phase: AttemptQuotaPhase,
    limits: &AttemptQuotaLimits,
) -> Result<(), LeaseError> {
    let age = unseen_attempt_age(reservation)?;
    let limit = match phase {
        AttemptQuotaPhase::Held => {
            Duration::from_secs(u64::from(limits.max_lease_lifetime_seconds.max(MAX_LEASE_TTL_SECONDS)) * 2)
        }
        AttemptQuotaPhase::Released => QUOTA_RETENTION,
    };
    if age >= limit {
        return Err(attempt_error(
            "quota attempt admission or receipt retention window has expired",
        ));
    }
    Ok(())
}

pub(super) fn reserve_attempt_budget(
    record: &mut QuotaRecord,
    reservation: &AttemptQuotaReservation,
) -> Result<(), LeaseError> {
    match reservation.accounting {
        LeaseAccounting::Tenant => {
            let cpu = record
                .cpu_seconds_used
                .checked_add(reservation.cpu_seconds)
                .filter(|value| *value <= record.cap_cpu_seconds);
            let wall = record
                .wall_seconds_used
                .checked_add(reservation.wall_seconds)
                .filter(|value| *value <= record.cap_wall_seconds);
            let (Some(cpu), Some(wall)) = (cpu, wall) else {
                return Err(LeaseError::QuotaExceeded {
                    tenant: reservation.tenant.clone(),
                });
            };
            if record.active_leases >= record.active_lease_cap {
                return Err(LeaseError::TooManyActiveLeases {
                    tenant: reservation.tenant.clone(),
                    cap: record.active_lease_cap,
                });
            }
            record.cpu_seconds_used = cpu;
            record.wall_seconds_used = wall;
            record.active_leases += 1;
        }
        LeaseAccounting::Prewarm => {
            let cpu = record
                .prewarm
                .total_cpu_seconds
                .checked_add(u64::from(reservation.cpu_seconds))
                .filter(|value| *value <= record.prewarm.cpu_cap);
            let Some(cpu) = cpu else {
                return Err(LeaseError::PrewarmQuotaExceeded {
                    tenant: reservation.tenant.clone(),
                });
            };
            if record.prewarm.active_count >= record.prewarm.count_cap {
                return Err(LeaseError::PrewarmQuotaExceeded {
                    tenant: reservation.tenant.clone(),
                });
            }
            record.prewarm.active_count += 1;
            record.prewarm.total_cpu_seconds = cpu;
        }
    }
    Ok(())
}

pub(super) fn release_attempt_budget(
    record: &mut QuotaRecord,
    reservation: &AttemptQuotaReservation,
) -> Result<(), QuotaStoreError> {
    match reservation.accounting {
        LeaseAccounting::Tenant => {
            let cpu = record
                .cpu_seconds_used
                .checked_sub(reservation.cpu_seconds)
                .ok_or(QuotaStoreError::AttemptCpuInconsistent)?;
            let wall = record
                .wall_seconds_used
                .checked_sub(reservation.wall_seconds)
                .ok_or(QuotaStoreError::AttemptWallInconsistent)?;
            let active = record
                .active_leases
                .checked_sub(1)
                .ok_or(QuotaStoreError::AttemptActiveInconsistent)?;
            record.cpu_seconds_used = cpu;
            record.wall_seconds_used = wall;
            record.active_leases = active;
        }
        LeaseAccounting::Prewarm => {
            let cpu = record
                .prewarm
                .total_cpu_seconds
                .checked_sub(u64::from(reservation.cpu_seconds))
                .ok_or(QuotaStoreError::AttemptPrewarmCpuInconsistent)?;
            let active = record
                .prewarm
                .active_count
                .checked_sub(1)
                .ok_or(QuotaStoreError::AttemptPrewarmCountInconsistent)?;
            record.prewarm.total_cpu_seconds = cpu;
            record.prewarm.active_count = active;
        }
    }
    Ok(())
}
