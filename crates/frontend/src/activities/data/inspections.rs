//! Each dataset's inspection — its format, its version and the streams it
//! records — read once and shared by every surface that asks for it: the
//! datasets ledger, the rail, the Overview, the thumbnail camera, an
//! episode's robot. Inspecting a dataset costs Spiral a catalog round trip,
//! and before this each of them paid it again for the same answer.
//!
//! The answers last until the catalog is read again, which is when a dataset
//! can have changed under Studio. A failed read is not kept, so the next ask
//! tries again.
use crate::client::CommandError;
use crate::client::queries::{QueryDomain, QueryRefresh};
use dioxus::prelude::*;
use futures_util::future::{FutureExt as _, LocalBoxFuture, Shared};
use spiral_robotics_common::datasets::{DatasetInspection, DatasetSelection};
use std::{cell::RefCell, collections::HashMap, future::Future, rc::Rc};

/// One dataset's inspection, read or being read, which every asker awaits.
pub(super) type Inspecting =
    Shared<LocalBoxFuture<'static, Result<DatasetInspection, CommandError>>>;

/// The project's dataset inspections. A handle, provided once per project.
#[derive(Clone)]
pub(crate) struct DatasetInspections {
    read: Rc<RefCell<InspectionsRead>>,
    refresh: QueryRefresh,
}

/// The inspections asked for since the catalog was last invalidated, by
/// dataset, each read or being read.
#[derive(Default)]
struct InspectionsRead {
    invalidations: u64,
    inspections: HashMap<DatasetSelection, Inspecting>,
}

impl DatasetInspections {
    pub(crate) fn use_provider() -> Self {
        let refresh = QueryRefresh::current();
        use_context_provider(|| Self {
            read: Rc::default(),
            refresh,
        })
    }
    pub(super) fn current() -> Self {
        consume_context()
    }

    /// `dataset`'s inspection if it has been read since the catalog was,
    /// without asking: a view that mounts over a known answer shows it on
    /// its first frame.
    pub(super) fn known(&self, dataset: &DatasetSelection) -> Option<DatasetInspection> {
        let read = self.read.borrow();
        if read.invalidations != self.refresh.invalidations(QueryDomain::Catalog) {
            return None;
        }
        read.inspections
            .get(dataset)?
            .peek()?
            .as_ref()
            .ok()
            .cloned()
    }

    /// `dataset`'s inspection: the answer another surface already has or is
    /// waiting for, else `inspect` started now.
    pub(super) fn inspect<F>(
        &self,
        dataset: &DatasetSelection,
        inspect: impl FnOnce() -> F,
    ) -> Inspecting
    where
        F: Future<Output = Result<DatasetInspection, CommandError>> + 'static,
    {
        let invalidations = self.refresh.invalidations(QueryDomain::Catalog);
        let key = dataset.clone();
        let mut read = self.read.borrow_mut();
        if read.invalidations != invalidations {
            read.inspections.clear();
            read.invalidations = invalidations;
        }
        if let Some(inspecting) = read.inspections.get(&key) {
            return inspecting.clone();
        }
        let request = inspect();
        let forget = Rc::downgrade(&self.read);
        let failed = key.clone();
        let inspecting = async move {
            let answer = request.await;
            if answer.is_err()
                && let Some(read) = forget.upgrade()
            {
                let mut read = read.borrow_mut();
                if read.invalidations == invalidations {
                    read.inspections.remove(&failed);
                }
            }
            answer
        }
        .boxed_local()
        .shared();
        read.inspections.insert(key, inspecting.clone());
        inspecting
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiral_robotics_common::command_failure::CommandFailureCode;
    use spiral_robotics_common::datasets::{DatasetFormat, DatasetSummary};
    use std::cell::Cell;

    fn dataset(name: &str) -> DatasetSelection {
        DatasetSelection::new(name).unwrap()
    }
    fn inspection(name: &str, version: &str) -> DatasetInspection {
        DatasetInspection::new(
            DatasetSummary::new(
                dataset(name),
                DatasetFormat::Mcap,
                ("resource".into(), version.into()),
            ),
            1,
            Vec::new(),
        )
    }

    /// The project's handles, as the provider made them.
    #[derive(Clone, Default)]
    struct Provided(Rc<RefCell<Option<(DatasetInspections, QueryRefresh)>>>);
    impl Provided {
        fn render(self) -> Element {
            let refresh = QueryRefresh::use_provider();
            let inspections = DatasetInspections::use_provider();
            *self.0.borrow_mut() = Some((inspections, refresh));
            rsx! {}
        }
        fn handles(&self) -> (DatasetInspections, QueryRefresh) {
            self.0.borrow().clone().expect("rendered")
        }
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn every_asker_shares_one_read_until_the_catalog_is_read_again() {
        let provided = Provided::default();
        let mut dom = VirtualDom::new_with_props(Provided::render, provided.clone());
        dom.rebuild_in_place();
        let (inspections, refresh) = provided.handles();
        let reads = Rc::new(Cell::new(0));
        let ask = |version: &'static str| {
            let reads = reads.clone();
            dom.in_runtime(|| {
                inspections.inspect(&dataset("a"), move || {
                    reads.set(reads.get() + 1);
                    async move { Ok(inspection("a", version)) }
                })
            })
        };
        let (first, second) = (ask("v1"), ask("v1"));
        assert_eq!(first.await, second.await);
        assert_eq!(reads.get(), 1);
        let known = dom.in_runtime(|| inspections.known(&dataset("a")));
        assert_eq!(known, Some(inspection("a", "v1")));

        dom.in_runtime(|| refresh.invalidate(&[QueryDomain::Catalog]));
        assert_eq!(dom.in_runtime(|| inspections.known(&dataset("a"))), None);
        assert_eq!(ask("v2").await, Ok(inspection("a", "v2")));
        assert_eq!(reads.get(), 2);
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn a_failed_read_is_not_kept() {
        let provided = Provided::default();
        let mut dom = VirtualDom::new_with_props(Provided::render, provided.clone());
        dom.rebuild_in_place();
        let (inspections, _) = provided.handles();
        let failing = dom.in_runtime(|| {
            inspections.inspect(&dataset("b"), || async {
                Err(CommandError::new(
                    CommandFailureCode::Transport,
                    "unreachable",
                ))
            })
        });
        assert!(failing.await.is_err());
        let retried = dom.in_runtime(|| {
            inspections.inspect(&dataset("b"), || async { Ok(inspection("b", "v1")) })
        });
        assert_eq!(retried.await, Ok(inspection("b", "v1")));
    }
}
