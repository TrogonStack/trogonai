use super::lease_release_error::LeaseReleaseError;
use super::*;

impl BrokerCore {
    /// Permanently deletes a lease: does everything [`Self::handle_cancel`]
    /// does (broadcast provider_cancel, mark Released, refund quota, clear
    /// drive attached_lease) plus purges anything that would let this exact
    /// lease's snapshot be resumed later. This is the differentiator from
    /// plain cancel, which preserves resumable state on purpose.
    ///
    /// A pending-purge marker is written before the release and dropped only
    /// once the purge is complete, so a delete that fails after the release
    /// can be retried by id, and the reconciler finishes it otherwise.
    pub async fn handle_delete_lease(&self, req: DeleteLeaseRequest) -> DeleteLeaseResponse {
        let Some(operations) = self.operations.clone() else {
            return self.delete_lease_inline(req).await;
        };
        match self
            .run_operation(
                &operations,
                &req.tenant,
                &req.lease_id,
                req.operation_id,
                OperationRequest::Delete,
            )
            .await
        {
            Ok((operation_id, OperationResult::Deleted)) => DeleteLeaseResponse::Deleted {
                operation_id: Some(operation_id),
            },
            Ok((_, other)) => DeleteLeaseResponse::Denied(unexpected_result(&other)),
            Err(error) => DeleteLeaseResponse::Denied(error),
        }
    }

    pub(crate) async fn delete_lease_inline(&self, req: DeleteLeaseRequest) -> DeleteLeaseResponse {
        let record = match self.active_leases.get(&req.lease_id).await {
            Ok(Some(r)) => r,
            Ok(None) => return self.retry_pending_delete(req).await,
            Err(e) => {
                warn!(error = %e, "delete lease denied: active lease lookup failed");
                return DeleteLeaseResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        if record.tenant != req.tenant {
            warn!(lease_id = %req.lease_id, "delete lease denied: tenant mismatch");
            return DeleteLeaseResponse::Denied(LeaseError::Unauthorized {
                reason: "lease does not belong to tenant".to_string(),
            });
        }

        // The purge may only remove the tag as it stands now: anything set
        // under the name later belongs to whoever set it.
        let tag_revision = match self.current_tag_revision(&record).await {
            Ok(revision) => revision,
            Err(e) => {
                warn!(error = %e, lease_id = %req.lease_id, "delete lease denied: snapshot tag lookup failed");
                return DeleteLeaseResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };
        let marker = PendingPurge {
            quota_reservation: record.quota_reservation.clone(),
            lease_id: req.lease_id.clone(),
            tenant: record.tenant.clone(),
            name: record.name.as_ref().map(ToString::to_string),
            tag_revision,
            release_announced: Some(false),
            // A release through a dispatch attempt refunds through the
            // attempt's own ledger, which is safe to repeat.
            pending_refund: record.dispatch_attempt.is_none().then_some(PendingRefund {
                cpu_seconds: record.cpu_seconds_consumed,
                wall_seconds: record.wall_seconds_consumed,
            }),
            drive_claims: Some(
                record
                    .spec
                    .drives()
                    .iter()
                    .map(|mount| mount.drive_id.clone())
                    .collect(),
            ),
            announcement: Some(announcement(&record, LeaseReleasedReason::Cancelled)),
            recorded_at: SystemTime::now(),
        };
        if let Err(e) = self.pending_purges.put(&marker).await {
            warn!(error = %e, lease_id = %req.lease_id, "delete lease denied: pending purge marker write failed");
            return DeleteLeaseResponse::Denied(LeaseError::Backend { message: e.to_string() });
        }

        if let Err(e) = self.cancel_lease(req.lease_id.clone(), CancelIntent::Purge).await {
            warn!(error = %e, lease_id = %req.lease_id, "delete lease denied: cancel failed");
            return DeleteLeaseResponse::Denied(LeaseError::Backend { message: e.to_string() });
        }
        let marker = self.mark_release_announced(marker).await;

        match self.finish_purge(&marker).await {
            Ok(()) => DeleteLeaseResponse::Deleted { operation_id: None },
            Err(e) => DeleteLeaseResponse::Denied(e),
        }
    }

    /// Whether a delete-by-id is purging `lease_id`. A lookup failure reads as
    /// not pending: the marker only narrows what a name resolves to.
    pub(super) async fn purge_pending(&self, lease_id: &LeaseId) -> bool {
        match self.pending_purges.get(lease_id).await {
            Ok(marker) => marker.is_some(),
            Err(e) => {
                warn!(error = %e, lease_id = %lease_id, "pending purge lookup failed; treating the lease as not being purged");
                false
            }
        }
    }

    /// Records on the marker that the purge's release finished: it was
    /// announced with a `lease_released` event and refunded its quota. Only
    /// called once the release returned. Best effort: a stale marker costs a
    /// re-announce, which dedupes on its message id.
    async fn mark_release_announced(&self, mut marker: PendingPurge) -> PendingPurge {
        if marker.release_announced == Some(false) || marker.pending_refund.is_some() {
            marker.release_announced = marker.release_announced.map(|_| true);
            marker.pending_refund = None;
            if let Err(e) = self.pending_purges.put(&marker).await {
                warn!(error = %e, lease_id = %marker.lease_id, "delete: failed to record the release announcement on the pending purge marker");
            }
        }
        marker
    }

    /// Redoes the parts of a release that run after the active record is
    /// removed: the drive claims, then the quota refund. Both are safe to
    /// repeat. A claim that cannot be cleared fails the redo, so the marker
    /// stays and the next retry or reconciler pass tries again.
    async fn redo_released_cleanup(&self, marker: PendingPurge) -> Result<PendingPurge, LeaseReleaseError> {
        match &marker.drive_claims {
            Some(drives) => {
                self.try_release_drive_claims(&marker.lease_id, drives).await?;
            }
            // Written before markers carried the drives: the released lease
            // record is gone and nothing else says what it mounted.
            None => {
                warn!(lease_id = %marker.lease_id, "delete: marker predates the drive list, drive claims not re-released");
            }
        }
        self.redo_pending_refund(marker).await
    }

    /// Refunds the quota a purge's release owed when it stopped after
    /// removing the active record, which only the caller has checked is gone.
    /// Recording the refund is best effort: the marker is dropped when the
    /// purge finishes anyway.
    pub(super) async fn redo_pending_refund(
        &self,
        mut marker: PendingPurge,
    ) -> Result<PendingPurge, LeaseReleaseError> {
        if marker.pending_refund.is_none() {
            return Ok(marker);
        }
        // The quota store applies a lease's refund once, so this redo is safe
        // against the release that removed the active record having applied
        // it already, against another broker's redo, and against our own
        // repeat after a crash before the marker was updated.
        let refunded = if let Some(reservation) = &marker.quota_reservation {
            self.quota.release_attempt(reservation, &self.quota_limits()).await?
        } else if let Some(refund) = marker.pending_refund {
            self.quota
                .refund_release(
                    &marker.tenant,
                    &marker.lease_id,
                    marker.recorded_at,
                    refund.cpu_seconds,
                    refund.wall_seconds,
                )
                .await?
        } else {
            false
        };
        if refunded {
            info!(lease_id = %marker.lease_id, "delete: refunded a release that stopped before its quota refund");
        }
        marker.pending_refund = None;
        Ok(marker)
    }

    /// A purge whose release removed the active record but stopped before
    /// publishing `lease_released` leaves subscribers believing the lease is
    /// alive. Announces it from the event the marker carries.
    async fn announce_unannounced_release(&self, marker: PendingPurge) -> Result<PendingPurge, LeaseReleaseError> {
        if marker.release_announced != Some(false) {
            return Ok(marker);
        }
        // A marker written before it carried the event has nothing to
        // announce from; finishing the purge matters more than the event.
        if let Some(event) = &marker.announcement {
            info!(lease_id = %marker.lease_id, "delete: announcing a release that stopped before its lease_released event");
            self.announce_lease_released(event).await?;
        }
        Ok(self.mark_release_announced(marker).await)
    }

    /// The revision of the snapshot tag under the lease's name, if it has a
    /// name and the tag exists.
    async fn current_tag_revision(&self, record: &LeaseRecord) -> Result<Option<u64>, SnapshotTagStoreError> {
        let Some(name) = record.name.as_ref() else {
            return Ok(None);
        };
        // A name that is not a valid tag name cannot have a tag.
        let Ok(tag) = snapshot_tag(&record.tenant, name.as_str()) else {
            return Ok(None);
        };
        Ok(self.tag_store.get_entry(&tag).await?.map(|(_, revision)| revision))
    }

    /// A delete whose lease record is already gone. If an earlier delete of
    /// this lease released it but did not finish the purge, finish it now;
    /// otherwise the lease is unknown. A marker of another tenant reads as
    /// not found.
    async fn retry_pending_delete(&self, req: DeleteLeaseRequest) -> DeleteLeaseResponse {
        match self.pending_purges.get(&req.lease_id).await {
            Ok(Some(marker)) if marker.tenant == req.tenant => {
                info!(lease_id = %req.lease_id, "delete lease: finishing an earlier purge");
                let marker = match self.redo_released_cleanup(marker).await {
                    Ok(marker) => marker,
                    Err(e) => {
                        return DeleteLeaseResponse::Denied(LeaseError::Backend { message: e.to_string() });
                    }
                };
                let marker = match self.announce_unannounced_release(marker).await {
                    Ok(marker) => marker,
                    Err(e) => {
                        return DeleteLeaseResponse::Denied(LeaseError::Backend { message: e.to_string() });
                    }
                };
                match self.finish_purge(&marker).await {
                    Ok(()) => DeleteLeaseResponse::Deleted { operation_id: None },
                    Err(e) => DeleteLeaseResponse::Denied(e),
                }
            }
            Ok(_) => {
                warn!(lease_id = %req.lease_id, "delete lease denied: lease not found");
                DeleteLeaseResponse::Denied(LeaseError::LeaseNotFound { lease_id: req.lease_id })
            }
            Err(e) => {
                warn!(error = %e, lease_id = %req.lease_id, "delete lease denied: pending purge lookup failed");
                DeleteLeaseResponse::Denied(LeaseError::Backend { message: e.to_string() })
            }
        }
    }

    /// Removes what would let a released lease be restored, then drops its
    /// pending-purge marker. Safe to repeat: every step tolerates having
    /// already run.
    async fn finish_purge(&self, marker: &PendingPurge) -> Result<(), LeaseError> {
        self.clear_deleted_lease_state(&marker.lease_id, &marker.tenant, marker.name.as_deref())
            .await;

        // A named lease leaves a snapshot tag under its name; without removing
        // it, a sandbox deleted by id could still be restored by name.
        if let Some(name) = marker.name.as_ref() {
            self.delete_purged_lease_tag(marker, name).await?;
        }

        if let Err(e) = self.pending_purges.clear(&marker.lease_id).await {
            warn!(error = %e, lease_id = %marker.lease_id, "delete: failed to drop the pending purge marker");
        }
        info!(lease_id = %marker.lease_id, "lease deleted");
        Ok(())
    }

    /// Deletes the snapshot tag a deleted lease left under its name, retrying
    /// a failing tag store with a short backoff. Only the revision recorded
    /// when the delete started is removed, with a revision-checked clear: a
    /// tag set under the name since then, by a newer lease (live or not) or
    /// anyone else, is kept.
    async fn delete_purged_lease_tag(&self, marker: &PendingPurge, name: &str) -> Result<(), LeaseError> {
        let Some(revision) = marker.tag_revision else {
            return Ok(());
        };
        let tag = snapshot_tag(&marker.tenant, name).map_err(|e| LeaseError::InvalidTags { reason: e.to_string() })?;

        let mut backoff = PURGE_TAG_DELETE_BACKOFF;
        let mut attempt = 1;
        loop {
            match self.tag_store.clear(&tag, revision).await {
                Ok(()) | Err(SnapshotTagStoreError::NotFound) => return Ok(()),
                Err(SnapshotTagStoreError::RevisionConflict) => {
                    info!(lease_id = %marker.lease_id, name = %name, "delete: snapshot tag was set again after the delete started, keeping it");
                    return Ok(());
                }
                Err(e) if attempt >= PURGE_TAG_DELETE_ATTEMPTS => {
                    warn!(error = %e, lease_id = %marker.lease_id, name = %name, "delete denied: snapshot tag still present; retry the delete or wait for the reconciler");
                    return Err(LeaseError::Backend { message: e.to_string() });
                }
                Err(e) => {
                    warn!(error = %e, lease_id = %marker.lease_id, name = %name, attempt, "delete: snapshot tag removal failed, retrying");
                    tokio::time::sleep(backoff).await;
                    backoff *= 2;
                    attempt += 1;
                }
            }
        }
    }

    /// Finishes delete-by-id purges that were released but never completed:
    /// the client gave up on its retry, or the broker stopped midway. Markers
    /// younger than [`PENDING_PURGE_RECONCILE_AFTER`] are left to the request
    /// that wrote them.
    pub async fn reconcile_pending_purges(&self) {
        let markers = match self.pending_purges.all().await {
            Ok(markers) => markers,
            Err(error) => {
                warn!(%error, "pending purge listing failed");
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
            let marker = match self.active_leases.get(&marker.lease_id).await {
                Ok(Some(record)) if record.status != LeaseStatus::Released => {
                    if let Err(error) = self.cancel_lease(marker.lease_id.clone(), CancelIntent::Purge).await {
                        warn!(lease_id = %marker.lease_id, %error, "pending purge: release still failing");
                        continue;
                    }
                    self.mark_release_announced(marker).await
                }
                Ok(Some(_)) => marker,
                Ok(None) => match self.redo_released_cleanup(marker).await {
                    Ok(marker) => match self.announce_unannounced_release(marker).await {
                        Ok(marker) => marker,
                        Err(error) => {
                            warn!(%error, "pending purge: lease_released announcement still failing");
                            continue;
                        }
                    },
                    Err(error) => {
                        warn!(%error, "pending purge: quota refund still failing");
                        continue;
                    }
                },
                Err(error) => {
                    warn!(lease_id = %marker.lease_id, %error, "pending purge: lease lookup failed");
                    continue;
                }
            };
            if let Err(error) = self.finish_purge(&marker).await {
                warn!(lease_id = %marker.lease_id, ?error, "pending purge remains unfinished");
            }
        }
    }

    /// Clears everything that would let a since-purged lease's snapshot be
    /// resumed later: the pinned expected digest, the resolved-digest cache,
    /// and the named-lease index entry. Shared by [`Self::handle_delete_lease`]
    /// and [`Self::handle_delete_named_sandbox`] so the two purge paths (by
    /// lease id, by name) cannot drift apart.
    pub(super) async fn clear_deleted_lease_state(&self, lease_id: &LeaseId, tenant: &TenantId, name: Option<&str>) {
        if let Err(e) = self.snapshot_pins.unpin(lease_id).await {
            warn!(error = %e, lease_id = %lease_id, "delete: failed to unpin expected snapshot digest");
        }

        if let Err(e) = self.resolved_digests.clear(lease_id).await {
            warn!(error = %e, lease_id = %lease_id, "delete: failed to clear resolved digest record");
        }

        if let Some(name) = name
            && let Err(e) = self.named_leases.clear_if_matches(tenant, name, lease_id).await
        {
            warn!(error = %e, lease_id = %lease_id, name = %name, "delete: failed to clear named lease index");
        }
    }

    /// Permanently deletes a sandbox by its tenant-scoped `name`, even when it
    /// has no active lease. Unlike [`Self::handle_delete_lease`], which needs a
    /// running lease to find anything, this looks the name up in every place a
    /// stopped named sandbox can still be found: the `named_leases` index (if
    /// somehow still running), the snapshot tag store, and finally the
    /// substrate worker fleet's suspended actor. `Deleted` if anything at all
    /// was found and removed, `NotFound` if nothing was, `Denied` on error.
    pub async fn handle_delete_named_sandbox(&self, req: DeleteNamedSandboxRequest) -> DeleteNamedSandboxResponse {
        let tag_name = match SnapshotTagName::new(req.name.as_str()) {
            Ok(name) => name,
            Err(e) => {
                warn!(error = %e, tenant = %req.tenant, name = %req.name, "delete_named_sandbox denied: invalid name");
                return DeleteNamedSandboxResponse::Denied(LeaseError::InvalidTags { reason: e.to_string() });
            }
        };

        if let Err(e) = self
            .entity_intend_delete(
                &req.tenant,
                &trogon_sandbox_contract::EntityRef::Name(req.name.to_string()),
            )
            .await
        {
            return DeleteNamedSandboxResponse::Denied(e);
        }

        let mut found_anything = false;

        match self.resolve_named_lease(&req.tenant, req.name.as_str()).await {
            Ok(Some(lease_id)) => match self.active_leases.get(&lease_id).await {
                Ok(Some(record)) if record.tenant == req.tenant && record.status != LeaseStatus::Released => {
                    found_anything = true;
                    if let Err(e) = self.cancel_lease(lease_id.clone(), CancelIntent::Purge).await {
                        warn!(error = %e, lease_id = %lease_id, tenant = %req.tenant, name = %req.name, "delete_named_sandbox denied: cancel failed");
                        return DeleteNamedSandboxResponse::Denied(LeaseError::Backend { message: e.to_string() });
                    }
                    self.clear_deleted_lease_state(&lease_id, &req.tenant, Some(req.name.as_str()))
                        .await;
                }
                Ok(_) => {
                    // Stale or foreign index entry: nothing here belongs to this delete.
                }
                Err(e) => {
                    warn!(error = %e, tenant = %req.tenant, name = %req.name, "delete_named_sandbox denied: active lease lookup failed");
                    return DeleteNamedSandboxResponse::Denied(LeaseError::Backend { message: e.to_string() });
                }
            },
            Ok(None) => {}
            Err(e) => {
                warn!(error = %e, tenant = %req.tenant, name = %req.name, "delete_named_sandbox denied: named lease lookup failed");
                return DeleteNamedSandboxResponse::Denied(e);
            }
        }

        match self
            .handle_snapshot_tag_delete(DeleteSnapshotTagRequest {
                tenant: req.tenant.clone(),
                name: tag_name,
            })
            .await
        {
            DeleteSnapshotTagResponse::Applied => found_anything = true,
            DeleteSnapshotTagResponse::NotFound => {}
            DeleteSnapshotTagResponse::Denied(e) => {
                warn!(tenant = %req.tenant, name = %req.name, error = %e, "delete_named_sandbox denied: snapshot tag delete failed");
                return DeleteNamedSandboxResponse::Denied(e);
            }
        }

        let purge_req = PurgeNamedRequest {
            tenant: req.tenant.clone(),
            name: req.name.clone(),
        };
        let body = match serde_json::to_vec(&purge_req) {
            Ok(b) => Bytes::from(b),
            Err(e) => {
                return DeleteNamedSandboxResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };
        let subject = subjects::provider_purge_named(ProviderKind::Substrate);
        match self.bus.request_with_timeout(&subject, body, PURGE_NAMED_TIMEOUT).await {
            Ok(reply) => match serde_json::from_slice::<PurgeNamedResponse>(&reply) {
                Ok(PurgeNamedResponse::Purged) => found_anything = true,
                Ok(PurgeNamedResponse::NotFound) => {}
                Ok(PurgeNamedResponse::Failed { message }) => {
                    warn!(tenant = %req.tenant, name = %req.name, %message, "delete_named_sandbox denied: provider purge failed");
                    return DeleteNamedSandboxResponse::Denied(LeaseError::Backend { message });
                }
                Err(e) => {
                    warn!(tenant = %req.tenant, name = %req.name, error = %e, "delete_named_sandbox denied: provider reply decode failed");
                    return DeleteNamedSandboxResponse::Denied(LeaseError::Backend {
                        message: format!("purge_named reply decode failed: {e}"),
                    });
                }
            },
            Err(e) if is_no_responders(&e) => {
                // No substrate workers deployed: nothing there to purge either.
            }
            Err(e) => {
                warn!(error = %e, tenant = %req.tenant, name = %req.name, "delete_named_sandbox denied: provider dispatch failed");
                return DeleteNamedSandboxResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        }

        match self.entity_deleted(&req.tenant, req.name.as_str()).await {
            Ok(removed) => found_anything |= removed,
            Err(e) => return DeleteNamedSandboxResponse::Denied(e),
        }

        if found_anything {
            info!(tenant = %req.tenant, name = %req.name, "named sandbox deleted");
            DeleteNamedSandboxResponse::Deleted
        } else {
            DeleteNamedSandboxResponse::NotFound
        }
    }
}

/// The tag `name` names in `tenant`.
fn snapshot_tag(tenant: &TenantId, name: &str) -> Result<SnapshotTag, trogon_sandbox_contract::SnapshotTagNameError> {
    Ok(SnapshotTag {
        tenant: tenant.clone(),
        name: SnapshotTagName::new(name)?,
    })
}
