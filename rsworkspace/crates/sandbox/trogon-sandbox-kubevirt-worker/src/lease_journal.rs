use std::sync::Arc;

use trogon_sandbox_contract::{LeaseId, TenantId};

use crate::constants::{CUSTODY_MAX_RECORDS, CUSTODY_VERSION};
use crate::custody_files::require;
use crate::{
    BoundedName, CreateIntent, CustodyError, CustodyJournal, CustodyPhase, CustodyRecord, CustodyResourceKind,
    CustodyToken, LeaseResources,
};

/// The writer of one allocation's custody record. It fails once another allocation has replaced the record.
pub struct LeaseJournal {
    journal: Arc<CustodyJournal>,
    lease: LeaseId,
    generation: CustodyToken,
}

impl LeaseJournal {
    /// Starts the record of a new allocation for `lease`, refusing while a record for it exists.
    pub fn begin(journal: &Arc<CustodyJournal>, lease: &LeaseId, tenant: &TenantId) -> Result<Arc<Self>, CustodyError> {
        require(journal.get(lease)?.is_none(), CustodyError::NotSettled)?;
        require(
            journal.all()?.len() < CUSTODY_MAX_RECORDS,
            CustodyError::CapacityExhausted,
        )?;
        let binding = journal.binding();
        let record = CustodyRecord {
            version: CUSTODY_VERSION,
            lease_id: lease.clone(),
            worker: binding.worker.clone(),
            namespace: binding.namespace.clone(),
            generation: CustodyToken::generate(),
            tenant: tenant.clone(),
            phase: CustodyPhase::Preparing,
            resources: LeaseResources::default(),
            create: None,
        };
        journal.store(&record)?;
        Ok(Self::handle(journal, &record))
    }

    /// The handle that writes `record`'s allocation, refusing once another allocation replaces it.
    pub fn handle(journal: &Arc<CustodyJournal>, record: &CustodyRecord) -> Arc<Self> {
        Arc::new(Self::new(journal.clone(), record))
    }

    fn new(journal: Arc<CustodyJournal>, record: &CustodyRecord) -> Self {
        Self {
            journal,
            lease: record.lease_id.clone(),
            generation: record.generation,
        }
    }

    /// The current record of this allocation.
    pub fn record(&self) -> Result<CustodyRecord, CustodyError> {
        let record = self.journal.get(&self.lease)?.ok_or(CustodyError::RecordUnavailable)?;
        require(record.generation == self.generation, CustodyError::GenerationChanged)?;
        Ok(record)
    }

    pub fn set_phase(&self, phase: CustodyPhase) -> Result<(), CustodyError> {
        let mut record = self.record()?;
        require(
            phase != CustodyPhase::Ready || record.ready_identity(),
            CustodyError::IncompleteReady,
        )?;
        record.phase = phase;
        self.journal.store(&record)
    }

    /// Writes down that `name` is about to be created, before the cluster is asked to.
    pub fn before_create(&self, kind: CustodyResourceKind, name: &str) -> Result<(), CustodyError> {
        let mut record = self.record()?;
        require(record.create.is_none(), CustodyError::CreateUnresolved)?;
        record.create = Some(CreateIntent {
            kind,
            name: BoundedName::new(name)?,
            nonce: CustodyToken::generate(),
        });
        self.journal.store(&record)
    }

    /// Replaces the record's resources, resolving the create intent they satisfy.
    pub fn resources(&self, resources: &LeaseResources) -> Result<(), CustodyError> {
        let mut record = self.record()?;
        if let Some(create) = &record.create {
            let receipt = match create.kind {
                CustodyResourceKind::VirtualMachine => resources.vm.as_ref(),
                CustodyResourceKind::DataVolume => resources.data_volume.as_ref(),
            };
            if receipt.is_some_and(|receipt| receipt.name == create.name.as_str()) {
                record.create = None;
                if record.phase == CustodyPhase::Unresolved {
                    record.phase = CustodyPhase::Cleanup;
                }
            }
        }
        record.resources = resources.clone();
        self.journal.store(&record)
    }

    /// Removes the record once nothing it names is left in the cluster.
    pub fn finish(&self) -> Result<(), CustodyError> {
        let record = self.record()?;
        require(
            record.create.is_none() && record.resources.is_empty(),
            CustodyError::CleanupUnsettled,
        )?;
        self.journal.remove(&self.lease)
    }
}
