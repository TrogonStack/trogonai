use super::capture::Capture;
use super::constants::{MAX_ATTEMPTS, MAX_CAPTURES};
use super::custody_error::SnapshotCustodyError;
use super::record::{CustodyRecord, validate_name};
use super::store::SnapshotCustody;
use crate::{DaytonaApi, DaytonaOrganizationId, SandboxRecord, SnapshotRecord};

impl SnapshotCustody {
    /// Admits a capture of `source` under `name`: the source must be readable, open and in one account, and the name
    /// must be free. The intent is stored before Daytona is asked for anything.
    pub(crate) async fn begin(&self, api: &DaytonaApi, source: &str, name: &str) -> Result<(), SnapshotCustodyError> {
        validate_name(name)?;
        let sandbox = api.get_sandbox(source).await?;
        let organization = confirmed_organization(&sandbox, source)?;
        let mut stored = match self.load(source).await? {
            Some(stored) => stored,
            None => {
                self.create(CustodyRecord::new(
                    &self.origin,
                    source,
                    Some(organization.clone()),
                    false,
                ))
                .await?
            }
        };
        if stored.record.organization.as_deref() != Some(organization.as_str()) || stored.record.closed {
            return Err(SnapshotCustodyError::SourceClosedOrMoved);
        }
        if stored.record.captures.len() >= MAX_CAPTURES || stored.record.captures.contains_key(name) {
            return Err(SnapshotCustodyError::CaptureReserved);
        }
        let scoped = api.for_organization(&DaytonaOrganizationId::new(&organization)?)?;
        if scoped.find_snapshot(name).await?.is_some() {
            return Err(SnapshotCustodyError::NameExists);
        }
        stored.record.captures.insert(name.to_owned(), Capture::intent());
        self.save(stored).await?;
        Ok(())
    }

    /// Whether `snapshot` is the capture `name` of this record's source, in this record's account.
    pub(super) fn validate_snapshot(
        record: &CustodyRecord,
        name: &str,
        snapshot: &SnapshotRecord,
        id: Option<&str>,
    ) -> Result<(), SnapshotCustodyError> {
        let owned = !snapshot.id.is_empty()
            && id.is_none_or(|id| snapshot.id == id)
            && snapshot.name == name
            && !snapshot.general
            && snapshot.organization_id.is_some()
            && snapshot.organization_id == record.organization
            && snapshot.source_sandbox_id.as_deref() == Some(record.source.as_str());
        if owned {
            Ok(())
        } else {
            Err(SnapshotCustodyError::OwnershipChanged)
        }
    }

    /// Records that Daytona accepted the capture, once its answer names the same source in the same account.
    pub(crate) async fn acknowledge(
        &self,
        source: &str,
        name: &str,
        answer: &SandboxRecord,
    ) -> Result<(), SnapshotCustodyError> {
        for _ in 0..MAX_ATTEMPTS {
            let mut stored = self.load(source).await?.ok_or(SnapshotCustodyError::Missing)?;
            if answer.id != source || answer.organization_id != stored.record.organization {
                return Err(SnapshotCustodyError::ForeignCreate);
            }
            let capture = stored
                .record
                .captures
                .get_mut(name)
                .ok_or(SnapshotCustodyError::IntentMissing)?;
            if !capture.pre_create_absent || capture.kept || capture.deleted {
                return Err(SnapshotCustodyError::NotEligible);
            }
            if capture.create_acknowledged {
                return Ok(());
            }
            capture.create_acknowledged = true;
            if self.save(stored).await.is_ok() {
                return Ok(());
            }
        }
        Err(SnapshotCustodyError::AcknowledgementLost)
    }

    /// Binds the snapshot's id to its capture. With `keep` the capture is also marked published, which a closed
    /// source refuses while still retaining the binding for cleanup.
    pub(crate) async fn observe(
        &self,
        source: &str,
        name: &str,
        snapshot: &SnapshotRecord,
        keep: bool,
    ) -> Result<(), SnapshotCustodyError> {
        for _ in 0..MAX_ATTEMPTS {
            let mut stored = self.load(source).await?.ok_or(SnapshotCustodyError::Missing)?;
            let closed = stored.record.closed;
            let capture = stored
                .record
                .captures
                .get(name)
                .ok_or(SnapshotCustodyError::IntentMissing)?;
            Self::validate_snapshot(&stored.record, name, snapshot, capture.id.as_deref())?;
            if capture.deleted || capture.kept {
                return Err(SnapshotCustodyError::AlreadyStarted);
            }
            if capture.id.as_deref() == Some(snapshot.id.as_str()) && !keep {
                return Ok(());
            }
            let capture = stored
                .record
                .captures
                .get_mut(name)
                .ok_or(SnapshotCustodyError::IntentMissing)?;
            capture.id = Some(snapshot.id.clone());
            capture.kept = keep && !closed;
            if self.save(stored).await.is_ok() {
                if keep && closed {
                    return Err(SnapshotCustodyError::PublicationRefused);
                }
                return Ok(());
            }
        }
        Err(SnapshotCustodyError::BindingLost)
    }

    /// The API scoped to the account custody retained for `source`.
    pub(crate) async fn scoped_api(&self, api: &DaytonaApi, source: &str) -> Result<DaytonaApi, SnapshotCustodyError> {
        let stored = self.load(source).await?.ok_or(SnapshotCustodyError::Missing)?;
        let organization = stored
            .record
            .organization
            .ok_or(SnapshotCustodyError::SourceUnconfirmed)?;
        Ok(api.for_organization(&DaytonaOrganizationId::new(&organization)?)?)
    }
}

/// The organization a source sandbox answer names, once the answer is about the source asked for.
pub(super) fn confirmed_organization(sandbox: &SandboxRecord, source: &str) -> Result<String, SnapshotCustodyError> {
    match sandbox.organization_id.as_deref() {
        Some(organization) if sandbox.id == source && !organization.is_empty() => Ok(organization.to_owned()),
        _ => Err(SnapshotCustodyError::SourceUnconfirmed),
    }
}
