//! One in-memory registry of admitted native runs, shared by every activity
//! that submits bounded queries to Spiral and lets the browser poll them:
//! snapshots by run id, one active task per document, and a shutdown that
//! waits for the work it admitted.
use crate::sync::MutexRecovery;
use std::{collections::BTreeMap, sync::Mutex};

pub(crate) struct RunRegistry<Record> {
    state: Mutex<RegistryState<Record>>,
}

struct RegistryState<Record> {
    snapshots: BTreeMap<String, Record>,
    active: BTreeMap<String, ActiveRun>,
}

struct ActiveRun {
    document_id: String,
    task: Option<tokio::task::JoinHandle<()>>,
}

/// One run about to be admitted: its identity, the document it judges, and
/// the record the browser will poll.
pub(crate) struct RunAdmission<Record> {
    run_id: String,
    document_id: String,
    record: Record,
}

impl<Record> RunAdmission<Record> {
    pub(crate) fn new(run_id: String, document_id: String, record: Record) -> Self {
        Self {
            run_id,
            document_id,
            record,
        }
    }
}

impl<Record: Clone> RunRegistry<Record> {
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(RegistryState {
                snapshots: BTreeMap::new(),
                active: BTreeMap::new(),
            }),
        }
    }
    pub(crate) fn all(&self) -> Vec<Record> {
        self.state
            .recover_lock()
            .snapshots
            .values()
            .cloned()
            .collect()
    }
    pub(crate) fn get(&self, run_id: &str) -> Option<Record> {
        self.state.recover_lock().snapshots.get(run_id).cloned()
    }
    pub(crate) fn is_active(&self) -> bool {
        !self.state.recover_lock().active.is_empty()
    }
    #[cfg(test)]
    pub(crate) fn active_count(&self) -> usize {
        self.state.recover_lock().active.len()
    }
    /// Admit under one lock: a document with an active run is refused with
    /// that run's id; otherwise the record is published and the task spawned
    /// before anyone can observe the gap between them.
    pub(crate) fn admit(
        &self,
        admission: RunAdmission<Record>,
        spawn: impl FnOnce() -> tokio::task::JoinHandle<()>,
    ) -> Result<(), String> {
        let mut state = self.state.recover_lock();
        if let Some((existing, _)) = state
            .active
            .iter()
            .find(|(_, active)| active.document_id == admission.document_id)
        {
            return Err(existing.clone());
        }
        state
            .snapshots
            .insert(admission.run_id.clone(), admission.record);
        let task = spawn();
        state.active.insert(
            admission.run_id,
            ActiveRun {
                document_id: admission.document_id,
                task: Some(task),
            },
        );
        Ok(())
    }
    pub(crate) fn update(&self, run_id: &str, change: impl FnOnce(&mut Record)) {
        if let Some(record) = self.state.recover_lock().snapshots.get_mut(run_id) {
            change(record);
        }
    }
    /// Apply the terminal change only while the run is still active, then
    /// release it; a run abandoned at shutdown keeps its last published state.
    pub(crate) fn finish(&self, run_id: &str, change: impl FnOnce(&mut Record)) {
        let mut state = self.state.recover_lock();
        if state.active.remove(run_id).is_none() {
            return;
        }
        if let Some(record) = state.snapshots.get_mut(run_id) {
            change(record);
        }
    }
    pub(crate) async fn shutdown(&self) {
        let tasks = {
            let mut state = self.state.recover_lock();
            state
                .active
                .values_mut()
                .filter_map(|active| active.task.take())
                .collect::<Vec<_>>()
        };
        for task in tasks {
            let _ = task.await;
        }
    }
}
