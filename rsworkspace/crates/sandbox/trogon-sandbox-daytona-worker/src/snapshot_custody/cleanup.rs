use super::capture_flow::confirmed_organization;
use super::constants::MAX_ATTEMPTS;
use super::custody_error::SnapshotCustodyError;
use super::record::CustodyRecord;
use super::store::SnapshotCustody;
use super::stored::Stored;
use crate::{DaytonaApi, DaytonaOrganizationId};

impl SnapshotCustody {
    /// Closes `source` to new captures, then deletes every capture that was not published. Returns the API scoped to
    /// the source's account, ready to delete the source itself.
    ///
    /// A capture Daytona accepted but never named is bound by its exact name first; one whose acceptance was never
    /// seen stays quarantined and the cleanup fails rather than guess.
    pub(crate) async fn cleanup(&self, api: &DaytonaApi, source: &str) -> Result<DaytonaApi, SnapshotCustodyError> {
        let mut stored = self.close(api, source).await?;
        let api = self.verified_api(api, &stored.record).await?;
        for (name, capture) in stored.record.captures.clone() {
            if capture.kept || capture.deleted {
                continue;
            }
            let id = match capture.id {
                Some(id) => id,
                None => {
                    if !(capture.pre_create_absent && capture.create_acknowledged) {
                        return Err(SnapshotCustodyError::Quarantined);
                    }
                    let snapshot = api
                        .find_snapshot(&name)
                        .await?
                        .ok_or(SnapshotCustodyError::NotYetVisible)?;
                    Self::validate_snapshot(&stored.record, &name, &snapshot, None)?;
                    self.observe(source, &name, &snapshot, false).await?;
                    stored = self.load(source).await?.ok_or(SnapshotCustodyError::Missing)?;
                    let bound = stored.record.captures.get(&name).is_some_and(|capture| {
                        capture.id.as_deref() == Some(snapshot.id.as_str()) && !capture.kept && !capture.deleted
                    });
                    if !stored.record.closed || !bound {
                        return Err(SnapshotCustodyError::LateBindingChanged);
                    }
                    snapshot.id
                }
            };
            if let Some(snapshot) = api.find_snapshot(&id).await? {
                Self::validate_snapshot(&stored.record, &name, &snapshot, Some(&id))?;
                api.delete_snapshot(&id).await?;
                if api.find_snapshot(&id).await?.is_some() {
                    return Err(SnapshotCustodyError::DeletionUnconfirmed);
                }
            }
            if let Some(capture) = stored.record.captures.get_mut(&name) {
                capture.deleted = true;
            }
            stored = self.save(stored).await?;
        }
        Ok(api)
    }

    /// The API to use when cleaning up a source that may already be gone: scoped to the retained account and checked
    /// against it when captures are at stake.
    pub(crate) async fn source_cleanup_api(
        &self,
        api: &DaytonaApi,
        source: &str,
    ) -> Result<DaytonaApi, SnapshotCustodyError> {
        let Some(stored) = self.load(source).await? else {
            return Ok(api.clone());
        };
        match stored.record.organization.as_deref() {
            Some(organization) if !organization.is_empty() => {
                let scoped = api.for_organization(&DaytonaOrganizationId::new(organization)?)?;
                if !stored.record.captures.is_empty() {
                    Self::verify_account(&scoped, &stored.record).await?;
                }
                Ok(scoped)
            }
            _ => Ok(api.clone()),
        }
    }

    /// Persists the close fence for `source`, creating the record when none exists.
    async fn close(&self, api: &DaytonaApi, source: &str) -> Result<Stored, SnapshotCustodyError> {
        for _ in 0..MAX_ATTEMPTS {
            match self.load(source).await? {
                Some(mut stored) => {
                    if stored.record.closed {
                        return Ok(stored);
                    }
                    stored.record.closed = true;
                    if let Ok(stored) = self.save(stored).await {
                        return Ok(stored);
                    }
                }
                None => {
                    let organization = match api.find_sandbox(source).await? {
                        Some(sandbox) => Some(confirmed_organization(&sandbox, source)?),
                        None => None,
                    };
                    let record = CustodyRecord::new(&self.origin, source, organization, true);
                    if let Ok(stored) = self.create(record).await {
                        return Ok(stored);
                    }
                }
            }
        }
        Err(SnapshotCustodyError::FenceLost)
    }

    async fn verified_api(&self, api: &DaytonaApi, record: &CustodyRecord) -> Result<DaytonaApi, SnapshotCustodyError> {
        let Some(organization) = record.organization.as_deref().filter(|id| !id.is_empty()) else {
            return Ok(api.clone());
        };
        let scoped = api.for_organization(&DaytonaOrganizationId::new(organization)?)?;
        if !record.captures.is_empty() {
            Self::verify_account(&scoped, record).await?;
        }
        Ok(scoped)
    }

    /// Proves the credentials still see the retained account: through the source if it exists, else through a retained
    /// snapshot, else through the organization itself.
    async fn verify_account(api: &DaytonaApi, record: &CustodyRecord) -> Result<(), SnapshotCustodyError> {
        if let Some(sandbox) = api.find_sandbox(&record.source).await? {
            if sandbox.id == record.source && sandbox.organization_id == record.organization {
                return Ok(());
            }
            return Err(SnapshotCustodyError::SourceMismatch);
        }
        for (name, capture) in &record.captures {
            if capture.deleted {
                continue;
            }
            if let Some(id) = capture.id.as_deref()
                && let Some(snapshot) = api.find_snapshot(id).await?
            {
                Self::validate_snapshot(record, name, &snapshot, Some(id))?;
                return Ok(());
            }
        }
        let organization = record.organization.as_deref().unwrap_or_default();
        api.verify_organization(&DaytonaOrganizationId::new(organization)?)
            .await?;
        Ok(())
    }
}
