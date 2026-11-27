use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use async_nats::jetstream;
use async_trait::async_trait;
use trogon_sandbox_contract::LaunchKey;

use crate::exec::command_custody_error::CommandCustodyError;
use crate::exec::command_owner::CommandOwner;
use crate::exec::command_record::CommandRecord;
use crate::exec::command_store::CommandStore;
use crate::exec::exec_error::ExecError;
use crate::exec::stored_command::StoredCommand;

/// The store operation a scripted fault applies to.
#[derive(Clone, Copy)]
pub(super) enum Op {
    Load,
    Create,
    Replace,
}

/// What the next call of one operation does instead of its ordinary behavior.
pub(super) enum Fault {
    Pass,
    Conflict,
    Fail,
    Rewrite(fn(&mut CommandRecord)),
}

#[derive(Default)]
struct State {
    entries: HashMap<String, (CommandRecord, u64)>,
    load: VecDeque<Fault>,
    create: VecDeque<Fault>,
    replace: VecDeque<Fault>,
    revision: u64,
}

impl State {
    fn faults(&mut self, op: Op) -> &mut VecDeque<Fault> {
        match op {
            Op::Load => &mut self.load,
            Op::Create => &mut self.create,
            Op::Replace => &mut self.replace,
        }
    }
}

/// An in-memory command store whose operations can be scripted to conflict or fail.
#[derive(Clone, Default)]
pub(super) struct ScriptedStore {
    state: Arc<Mutex<State>>,
}

fn key_of(owner: &CommandOwner, launch_key: &LaunchKey) -> String {
    format!("{}/{}/{}", owner.tenant, owner.lease_id, launch_key)
}

impl ScriptedStore {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(super) fn seed(&self, record: CommandRecord) {
        let mut state = self.state();
        state.revision += 1;
        let revision = state.revision;
        state
            .entries
            .insert(key_of(&record.owner, &record.launch_key), (record, revision));
    }

    pub(super) fn script(&self, op: Op, faults: impl IntoIterator<Item = Fault>) {
        self.state().faults(op).extend(faults);
    }

    pub(super) fn record(&self, owner: &CommandOwner, launch_key: &LaunchKey) -> Option<CommandRecord> {
        self.state()
            .entries
            .get(&key_of(owner, launch_key))
            .map(|(record, _)| record.clone())
    }
}

#[async_trait]
impl CommandStore for ScriptedStore {
    fn is_configured(&self) -> bool {
        true
    }

    fn configure(&self, _context: jetstream::Context) -> Result<(), ExecError> {
        Ok(())
    }

    async fn load(
        &self,
        owner: &CommandOwner,
        launch_key: &LaunchKey,
    ) -> Result<Option<StoredCommand>, CommandCustodyError> {
        let mut state = self.state();
        let key = key_of(owner, launch_key);
        match state.faults(Op::Load).pop_front() {
            Some(Fault::Fail) => return Err(CommandCustodyError::Changed),
            Some(Fault::Rewrite(rewrite)) => {
                if let Some((record, _)) = state.entries.get_mut(&key) {
                    rewrite(record);
                }
            }
            Some(Fault::Pass | Fault::Conflict) | None => {}
        }
        Ok(state.entries.get(&key).map(|(record, revision)| StoredCommand {
            record: record.clone(),
            revision: *revision,
        }))
    }

    async fn create(&self, record: CommandRecord) -> Result<Option<StoredCommand>, CommandCustodyError> {
        let mut state = self.state();
        match state.faults(Op::Create).pop_front() {
            Some(Fault::Fail) => return Err(CommandCustodyError::Changed),
            Some(Fault::Conflict) => return Ok(None),
            Some(Fault::Pass | Fault::Rewrite(_)) | None => {}
        }
        let key = key_of(&record.owner, &record.launch_key);
        if state.entries.contains_key(&key) {
            return Ok(None);
        }
        state.revision += 1;
        let revision = state.revision;
        state.entries.insert(key, (record.clone(), revision));
        Ok(Some(StoredCommand { record, revision }))
    }

    async fn replace(
        &self,
        expected: &StoredCommand,
        next: CommandRecord,
    ) -> Result<Option<StoredCommand>, CommandCustodyError> {
        let mut state = self.state();
        match state.faults(Op::Replace).pop_front() {
            Some(Fault::Fail) => return Err(CommandCustodyError::Changed),
            Some(Fault::Conflict) => return Ok(None),
            Some(Fault::Pass | Fault::Rewrite(_)) | None => {}
        }
        let key = key_of(&next.owner, &next.launch_key);
        if state.entries.get(&key).map(|(_, revision)| *revision) != Some(expected.revision) {
            return Ok(None);
        }
        state.revision += 1;
        let revision = state.revision;
        state.entries.insert(key, (next.clone(), revision));
        Ok(Some(StoredCommand { record: next, revision }))
    }
}
