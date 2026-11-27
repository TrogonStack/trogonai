use super::lease_release_error::LeaseReleaseError;
use super::lock::lock;
use super::*;

impl BrokerCore {
    #[tracing::instrument(skip_all, fields(lease_id = %lease_id))]
    pub async fn handle_cancel(&self, lease_id: LeaseId, reason: LeaseReleasedReason) -> Result<(), LeaseReleaseError> {
        self.release_lease(lease_id, CancelIntent::Release, reason).await
    }

    pub(super) async fn cancel_lease(&self, lease_id: LeaseId, intent: CancelIntent) -> Result<(), LeaseReleaseError> {
        self.release_lease(lease_id, intent, LeaseReleasedReason::Cancelled)
            .await
    }

    pub(super) async fn release_lease(
        &self,
        lease_id: LeaseId,
        intent: CancelIntent,
        reason: LeaseReleasedReason,
    ) -> Result<(), LeaseReleaseError> {
        // Prefer active-lease store as the source of truth for cancel operations.
        let record_opt = self.active_leases.get(&lease_id).await?;

        let record = record_opt.ok_or_else(|| LeaseReleaseError::NotFound(lease_id.clone()))?;

        if let Err(e) = self.usage.finish(&record, SystemTime::now()).await {
            warn!(error = %e, lease_id = %lease_id, "usage final stamp failed");
        }

        if record.dispatch_attempt.is_some() {
            return Ok(self.cancel_targeted(&record, intent, reason).await?);
        }

        let subject = trogon_sandbox_protocol::subjects::provider_cancel(record.provider, &lease_id);
        self.bus
            .publish(&subject, Bytes::from(serde_json::to_vec(&intent)?))
            .await
            .map_err(LeaseReleaseError::bus)?;

        // The marker is written before the record goes, so a broker that
        // stops after the removal leaves the refund, the claims and the
        // announcement to the reconciler.
        let (refund, month_of, release_marker) = self.release_marker(&record, intent, reason).await?;

        // Idempotent: only refund if we actually removed the active-lease record
        if self.active_leases.delete(&lease_id).await? {
            if let Some(reservation) = &record.quota_reservation {
                self.quota.release_attempt(reservation, &self.quota_limits()).await?;
            } else {
                self.quota
                    .refund_release(
                        &record.tenant,
                        &record.lease_id,
                        month_of,
                        refund.cpu_seconds,
                        refund.wall_seconds,
                    )
                    .await?;
            }
        } else if let Some((_, true)) = &release_marker {
            // Another release took the record and has its own marker; this
            // one was created after that release finished.
            self.clear_pending_release(&lease_id).await;
        }
        // Clearing a claim is exact-owner and repeatable, so it runs even
        // when another release removed the record: a release that stopped
        // after the removal but before this point left the claim behind.
        self.release_drive_claims(&record).await;
        self.revoke_credential_policy(&record.tenant, &lease_id).await;

        if let Some(name) = &record.name
            && let Err(e) = self
                .named_leases
                .clear_if_matches(&record.tenant, name.as_str(), &lease_id)
                .await
        {
            warn!(error = %e, tenant = %record.tenant, name = %name, lease_id = %lease_id, "failed to clear named lease index on cancel");
        }

        self.entity_run_ended(&record, &format!("{reason:?}")).await;

        let event = match &release_marker {
            Some((marker, _)) => marker.announcement.clone(),
            None => self
                .pending_purges
                .get(&lease_id)
                .await?
                .and_then(|marker| marker.announcement)
                .unwrap_or_else(|| announcement(&record, reason)),
        };
        self.announce_lease_released(&event).await?;
        if matches!(release_marker, Some((_, true))) {
            self.clear_pending_release(&lease_id).await;
        }

        info!(tenant = %record.tenant, "lease cancelled");

        self.metrics.dec_active_leases(record.tenant.as_str());
        self.purge_lease_command_state(&record).await;

        Ok(())
    }

    /// The refund a release owes and the month it lands in, plus the release
    /// marker written to make it durable. A purge refunds from its own
    /// pending-purge marker; every other release, and a purge without a
    /// marker, writes a release marker first. Both fix the amounts and the
    /// month, so the reconciler's redo, a retried delete, or another broker's
    /// release of the same lease finds the quota store's receipt and does
    /// nothing. The marker's flag says whether this call created it.
    async fn release_marker(
        &self,
        record: &LeaseRecord,
        intent: CancelIntent,
        reason: LeaseReleasedReason,
    ) -> Result<(PendingRefund, SystemTime, Option<(PendingRelease, bool)>), LeaseReleaseError> {
        if intent == CancelIntent::Purge
            && let Some(marker) = self.pending_purges.get(&record.lease_id).await?
        {
            let refund = marker.pending_refund.unwrap_or(PendingRefund {
                cpu_seconds: record.cpu_seconds_consumed,
                wall_seconds: record.wall_seconds_consumed,
            });
            return Ok((refund, marker.recorded_at, None));
        }
        let (marker, created) = self
            .pending_releases
            .create(&PendingRelease {
                quota_reservation: record.quota_reservation.clone(),
                lease_id: record.lease_id.clone(),
                tenant: record.tenant.clone(),
                name: record.name.as_ref().map(ToString::to_string),
                refund: PendingRefund {
                    cpu_seconds: record.cpu_seconds_consumed,
                    wall_seconds: record.wall_seconds_consumed,
                },
                drive_claims: record
                    .spec
                    .drives()
                    .iter()
                    .map(|mount| mount.drive_id.clone())
                    .collect(),
                announcement: announcement(record, reason),
                recorded_at: SystemTime::now(),
            })
            .await?;
        Ok((marker.refund, marker.recorded_at, Some((marker, created))))
    }

    /// Drops a release marker once its follow-up is done. Best effort: the
    /// reconciler redoes the follow-up, which is idempotent, and drops it.
    async fn clear_pending_release(&self, lease_id: &LeaseId) {
        if let Err(e) = self.pending_releases.clear(lease_id).await {
            warn!(error = %e, lease_id = %lease_id, "failed to drop the pending release marker");
        }
    }

    /// Finishes releases that removed the active record but stopped before
    /// their refund, drive claims or `lease_released` event, and drops the
    /// marker of a release that stopped before removing the record. Markers
    /// younger than [`PENDING_PURGE_RECONCILE_AFTER`] are left to the release
    /// that wrote them.
    pub async fn reconcile_pending_releases(&self) {
        let markers = match self.pending_releases.all().await {
            Ok(markers) => markers,
            Err(error) => {
                warn!(%error, "pending release listing failed");
                return;
            }
        };
        let now = SystemTime::now();
        for marker in markers {
            let settled = now
                .duration_since(marker.recorded_at)
                .is_ok_and(|age| age >= PENDING_PURGE_RECONCILE_AFTER);
            if !settled {
                continue;
            }
            match self.active_leases.get(&marker.lease_id).await {
                Ok(Some(_)) => {
                    // The release stopped before removing the record, so the
                    // lease is still live and nothing is owed.
                    self.clear_pending_release(&marker.lease_id).await;
                }
                Ok(None) => {
                    if let Err(error) = self.redo_release(&marker).await {
                        warn!(lease_id = %marker.lease_id, %error, "pending release: follow-up still failing");
                        continue;
                    }
                    self.clear_pending_release(&marker.lease_id).await;
                }
                Err(error) => {
                    warn!(lease_id = %marker.lease_id, %error, "pending release: lease lookup failed");
                }
            }
        }
    }

    /// Releases drive claims no lease backs: the lease has no active record,
    /// release or purge marker, or in-flight dispatch, and the claim is older
    /// than the grace period. A grant claims drives before it writes the
    /// active record, so age is the only evidence that separates a crashed
    /// grant from a running one. The release is guarded by the revision the
    /// claim was read at, so a claim re-written meanwhile is never removed.
    pub async fn reconcile_orphan_drive_claims(&self) {
        let after = lock(&self.orphan_claim_scan).cursor.clone();
        let page = match self
            .drive_store
            .list_claims(after.as_deref(), ORPHAN_CLAIM_SCAN_LIMIT)
            .await
        {
            Ok(page) => page,
            Err(error) => {
                warn!(%error, "orphan drive claim listing failed");
                return;
            }
        };
        let cycle = {
            let mut scan = lock(&self.orphan_claim_scan);
            scan.cursor = page.next.clone();
            if page.next.is_none() {
                scan.cycle += 1;
            }
            scan.cycle
        };
        let claims = page.claims;
        let now = SystemTime::now();
        let grace = self
            .orphan_claim_grace
            .max(self.config.provider_dispatch_timeout.saturating_mul(2));
        let first_seen = {
            let mut seen = lock(&self.orphan_claims_seen);
            let mut current = std::collections::HashMap::with_capacity(claims.len());
            for claim in &claims {
                let key = (claim.drive_id.clone(), claim.lease_id.clone(), claim.revision);
                let at = seen.get(&key).map_or(now, |(at, _)| *at);
                seen.insert(key.clone(), (at, cycle));
                current.insert(key, at);
            }
            // Every claim is revisited once per scan cycle, so one not
            // stamped in the last two cycles is gone or was rewritten.
            seen.retain(|_, (_, stamp)| stamp.saturating_add(2) > cycle);
            current
        };
        let aged: Vec<&DriveClaim> = claims
            .iter()
            .filter(|claim| {
                let key = (claim.drive_id.clone(), claim.lease_id.clone(), claim.revision);
                let since = claim.claimed_at.unwrap_or_else(|| first_seen[&key]);
                now.duration_since(since).is_ok_and(|age| age >= grace)
            })
            .collect();
        if aged.is_empty() {
            return;
        }
        let releasing: std::collections::HashSet<LeaseId> = match self.pending_releases.all().await {
            Ok(markers) => markers.into_iter().map(|marker| marker.lease_id).collect(),
            Err(error) => {
                warn!(%error, "orphan drive claims: pending release listing failed");
                return;
            }
        };
        let mut released = 0;
        for claim in aged {
            if released >= ORPHAN_CLAIM_RELEASES_PER_SWEEP {
                break;
            }
            let lease_id = &claim.lease_id;
            if releasing.contains(lease_id) || self.pending_redrives().contains_key(lease_id.as_str()) {
                continue;
            }
            match self.pending_purges.get(lease_id).await {
                Ok(None) => {}
                Ok(Some(_)) => continue,
                Err(error) => {
                    warn!(lease_id = %lease_id, %error, "orphan drive claims: purge marker lookup failed");
                    continue;
                }
            }
            match self.active_leases.get(lease_id).await {
                Ok(None) => {}
                Ok(Some(_)) => continue,
                Err(error) => {
                    warn!(lease_id = %lease_id, %error, "orphan drive claims: lease lookup failed");
                    continue;
                }
            }
            match self
                .drive_store
                .release_claim_at(&claim.drive_id, lease_id, claim.revision)
                .await
            {
                Ok(true) => {
                    released += 1;
                    info!(
                        drive = %claim.drive_id.name,
                        lease_id = %lease_id,
                        "released a drive claim no lease backs"
                    );
                }
                Ok(false) => {}
                Err(error) => {
                    warn!(drive = %claim.drive_id.name, lease_id = %lease_id, %error, "orphan drive claims: release failed");
                }
            }
        }
    }

    /// The steps a release runs after removing the active record, from its
    /// marker alone. Every step tolerates having run already.
    async fn redo_release(&self, marker: &PendingRelease) -> Result<(), LeaseReleaseError> {
        self.try_release_drive_claims(&marker.lease_id, &marker.drive_claims)
            .await?;
        let refunded = if let Some(reservation) = &marker.quota_reservation {
            self.quota.release_attempt(reservation, &self.quota_limits()).await?
        } else {
            self.quota
                .refund_release(
                    &marker.tenant,
                    &marker.lease_id,
                    marker.recorded_at,
                    marker.refund.cpu_seconds,
                    marker.refund.wall_seconds,
                )
                .await?
        };
        if refunded {
            info!(lease_id = %marker.lease_id, "refunded a release that stopped before its quota refund");
        }
        if let Some(name) = &marker.name
            && let Err(e) = self
                .named_leases
                .clear_if_matches(&marker.tenant, name, &marker.lease_id)
                .await
        {
            warn!(error = %e, lease_id = %marker.lease_id, "failed to clear named lease index on redo");
        }
        self.announce_lease_released(&marker.announcement).await
    }

    /// Clears the lease's claim on each drive it mounted. Idempotent and best
    /// effort: a failure is logged and the next release or delete retries it.
    async fn release_drive_claims(&self, record: &LeaseRecord) {
        let drives: Vec<trogon_sandbox_contract::DriveId> = record
            .spec
            .drives()
            .iter()
            .map(|mount| mount.drive_id.clone())
            .collect();
        for drive_id in &drives {
            if let Err(e) = self.drive_store.release_claim(drive_id, &record.lease_id).await {
                warn!(
                    error = %e,
                    drive = %drive_id.name,
                    lease_id = %record.lease_id,
                    "failed to clear drive attached_lease on release"
                );
            }
        }
    }

    /// [`Self::release_drive_claims`] for a redo that must not give up
    /// silently: tries every drive, then reports the first failure.
    pub(super) async fn try_release_drive_claims(
        &self,
        lease_id: &LeaseId,
        drives: &[trogon_sandbox_contract::DriveId],
    ) -> Result<(), LeaseReleaseError> {
        let mut first_failure = None;
        for drive_id in drives {
            if let Err(e) = self.drive_store.release_claim(drive_id, lease_id).await {
                warn!(error = %e, drive = %drive_id.name, lease_id = %lease_id, "failed to clear drive attached_lease on redo");
                first_failure.get_or_insert(e);
            }
        }
        first_failure.map_or(Ok(()), |e| Err(LeaseReleaseError::DriveClaimRelease(e)))
    }

    pub(super) async fn announce_lease_released(
        &self,
        release_payload: &LeaseReleased,
    ) -> Result<(), LeaseReleaseError> {
        let release_bytes = serde_json::to_vec(release_payload)?;
        let release_subject = trogon_sandbox_protocol::subjects::lease_released(&release_payload.lease_id);
        self.bus
            .publish_with_msg_id(
                &release_subject,
                &lease_released_msg_id(&release_payload.lease_id),
                Bytes::from(release_bytes),
            )
            .await
            .map_err(LeaseReleaseError::bus)?;
        Ok(())
    }

    /// Launch keys die with their lease, and a lease created with
    /// `retention.monitoring` off keeps its command history only while it
    /// runs. Purging is best effort: a failure is logged and the stores' own
    /// age limits still expire what is left.
    pub(super) async fn purge_lease_command_state(&self, record: &LeaseRecord) {
        if let Err(e) = self.command_launches.forget_lease(&record.lease_id).await {
            warn!(error = %e, lease_id = %record.lease_id, "failed to forget the launch keys of an ended lease");
        }
        if record.spec.retention().monitoring.is_retained() {
            return;
        }
        let Some(store) = &self.command_logs else {
            return;
        };
        if let Err(e) = store.purge_lease(&record.lease_id).await {
            warn!(error = %e, lease_id = %record.lease_id, "failed to purge command history of an unmonitored lease");
        }
    }
}
