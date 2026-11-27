use async_nats::jetstream::kv;
use bytes::Bytes;
use trogon_sandbox_contract::{AttemptQuotaReservation, DispatchAttemptId, LeaseError};

use super::NatsKvQuotaStore;
use crate::quota::attempt_budget::{
    attempt_error, attempt_month, new_attempt_record, release_attempt_budget, reserve_attempt_budget,
    validate_unseen_attempt,
};
use crate::quota::attempt_quota_limits::AttemptQuotaLimits;
use crate::quota::attempt_quota_receipt::{AttemptQuotaPhase, AttemptQuotaReceipt};
use crate::quota::pending_attempt_quota::PendingAttemptQuota;
use crate::quota::quota_keys::{legacy_quota_kv_key_for_month, quota_kv_key_for_month};
use crate::quota::quota_record::QuotaRecord;
use crate::quota::quota_store_error::QuotaStoreError;

impl NatsKvQuotaStore {
    pub(in crate::quota) fn attempt_receipt_key(attempt: &DispatchAttemptId) -> String {
        format!("attempt.{}", attempt.as_str())
    }

    pub(in crate::quota) async fn read_attempt_receipt(
        &self,
        attempt: &DispatchAttemptId,
    ) -> Result<Option<(AttemptQuotaReceipt, u64)>, QuotaStoreError> {
        match self.kv.entry(Self::attempt_receipt_key(attempt)).await? {
            Some(entry) if entry.operation == kv::Operation::Put => {
                Ok(Some((serde_json::from_slice(&entry.value)?, entry.revision)))
            }
            _ => Ok(None),
        }
    }

    pub(in crate::quota) async fn write_attempt_receipt(
        &self,
        desired: &AttemptQuotaReceipt,
    ) -> Result<(), QuotaStoreError> {
        let key = Self::attempt_receipt_key(&desired.reservation.attempt_id);
        let bytes = Bytes::from(serde_json::to_vec(desired)?);
        for _ in 0..self.cas_retries {
            match self.read_attempt_receipt(&desired.reservation.attempt_id).await? {
                Some((existing, revision)) => {
                    if existing.reservation != desired.reservation {
                        return Err(QuotaStoreError::AttemptIdentityReused);
                    }
                    if existing.phase == desired.phase || existing.phase == AttemptQuotaPhase::Released {
                        return Ok(());
                    }
                    match self.kv.update(&key, bytes.clone(), revision).await {
                        Ok(_) => return Ok(()),
                        Err(error) if error.kind() == kv::UpdateErrorKind::WrongLastRevision => {
                            continue;
                        }
                        Err(error) => return Err(error.into()),
                    }
                }
                None => match self.kv.create(&key, bytes.clone()).await {
                    Ok(_) => return Ok(()),
                    Err(error) if error.kind() == kv::CreateErrorKind::AlreadyExists => continue,
                    Err(error) => return Err(error.into()),
                },
            }
        }
        Err(QuotaStoreError::ReceiptCasRetriesExhausted)
    }

    pub(in crate::quota) async fn settle_attempt(
        &self,
        key: &str,
        pending: &PendingAttemptQuota,
    ) -> Result<(), QuotaStoreError> {
        self.write_attempt_receipt(&pending.receipt).await?;
        for _ in 0..self.settlement_cas_retries {
            let entry = self.kv.entry(key).await?.ok_or(QuotaStoreError::BudgetDisappeared)?;
            let mut record: QuotaRecord = serde_json::from_slice(&entry.value)?;
            if record.pending_attempt.as_ref() != Some(pending) {
                return Ok(());
            }
            let receipt = self
                .read_attempt_receipt(&pending.receipt.reservation.attempt_id)
                .await?
                .ok_or(QuotaStoreError::ReceiptDisappeared)?
                .0;
            if receipt != pending.receipt {
                return Err(QuotaStoreError::ReceiptAdvanced);
            }
            record.pending_attempt = None;
            match self
                .kv
                .update(key, serde_json::to_vec(&record)?.into(), entry.revision)
                .await
            {
                Ok(_) => return Ok(()),
                Err(error) if error.kind() == kv::UpdateErrorKind::WrongLastRevision => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err(QuotaStoreError::SettlementCasRetriesExhausted)
    }

    pub(in crate::quota) async fn transition_attempt(
        &self,
        reservation: &AttemptQuotaReservation,
        limits: &AttemptQuotaLimits,
        phase: AttemptQuotaPhase,
    ) -> Result<bool, LeaseError> {
        let (year, month) = attempt_month(reservation)?;
        let key = quota_kv_key_for_month(&reservation.tenant, year, month);
        let legacy = legacy_quota_kv_key_for_month(&reservation.tenant, year, month);
        for _ in 0..self.cas_retries {
            let entry = self
                .entry_seeded(&key, &legacy)
                .await
                .map_err(|error| attempt_error(error.to_string()))?;
            let (mut record, revision) = match entry {
                Some(entry) if entry.operation == kv::Operation::Put => (
                    serde_json::from_slice::<QuotaRecord>(&entry.value)
                        .map_err(|error| attempt_error(error.to_string()))?,
                    Some(entry.revision),
                ),
                _ => (new_attempt_record(limits), None),
            };
            if let Some(pending) = &record.pending_attempt {
                self.settle_attempt(&key, pending)
                    .await
                    .map_err(|error| attempt_error(error.to_string()))?;
                continue;
            }
            let previous = self
                .read_attempt_receipt(&reservation.attempt_id)
                .await
                .map_err(|error| attempt_error(error.to_string()))?;
            let held = match previous {
                Some((receipt, _)) => {
                    if receipt.reservation != *reservation {
                        return Err(attempt_error("quota attempt identity was reused"));
                    }
                    match (phase, receipt.phase) {
                        (AttemptQuotaPhase::Held, AttemptQuotaPhase::Held) => return Ok(false),
                        (AttemptQuotaPhase::Held, AttemptQuotaPhase::Released) => {
                            return Err(attempt_error("quota attempt was released before admission"));
                        }
                        (AttemptQuotaPhase::Released, AttemptQuotaPhase::Released) => {
                            return Ok(false);
                        }
                        (AttemptQuotaPhase::Released, AttemptQuotaPhase::Held) => true,
                    }
                }
                None => {
                    validate_unseen_attempt(reservation, phase, limits)?;
                    false
                }
            };
            match phase {
                AttemptQuotaPhase::Held => reserve_attempt_budget(&mut record, reservation)?,
                AttemptQuotaPhase::Released if held => release_attempt_budget(&mut record, reservation)
                    .map_err(|error| attempt_error(error.to_string()))?,
                AttemptQuotaPhase::Released => {}
            }
            let pending = PendingAttemptQuota {
                receipt: AttemptQuotaReceipt {
                    reservation: reservation.clone(),
                    phase,
                },
            };
            record.pending_attempt = Some(pending.clone());
            let bytes: Bytes = serde_json::to_vec(&record)
                .map_err(|error| attempt_error(error.to_string()))?
                .into();
            let committed = match revision {
                Some(revision) => match self.kv.update(&key, bytes, revision).await {
                    Ok(_) => true,
                    Err(error) if error.kind() == kv::UpdateErrorKind::WrongLastRevision => false,
                    Err(error) => return Err(attempt_error(error.to_string())),
                },
                None => match self.kv.create(&key, bytes).await {
                    Ok(_) => true,
                    Err(error) if error.kind() == kv::CreateErrorKind::AlreadyExists => false,
                    Err(error) => return Err(attempt_error(error.to_string())),
                },
            };
            if !committed {
                continue;
            }
            self.settle_attempt(&key, &pending)
                .await
                .map_err(|error| attempt_error(error.to_string()))?;
            if phase == AttemptQuotaPhase::Held {
                let current = self
                    .read_attempt_receipt(&reservation.attempt_id)
                    .await
                    .map_err(|error| attempt_error(error.to_string()))?
                    .ok_or_else(|| attempt_error("quota receipt is missing"))?
                    .0;
                if current.phase != AttemptQuotaPhase::Held {
                    return Err(attempt_error("quota attempt was released during admission"));
                }
            }
            return Ok(phase == AttemptQuotaPhase::Held || held);
        }
        Err(attempt_error("quota attempt CAS retries exhausted"))
    }
}
