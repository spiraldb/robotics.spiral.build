//! Durable state and commands for the Training screen.
//!
//! Training retains drafts and selection across route changes. Its reads run only
//! while Training is active.
//! Admitted jobs continue on the backend after the view is closed.

use crate::activities::training::TrainingClient;
use std::collections::BTreeSet;

use dioxus::prelude::*;
use spiral_robotics_common::training::{
    ActModel, ImageSize, ModelPublication, TrainingConfig, TrainingModel, TrainingModelKind,
    TrainingSource,
};

use super::observation::{ImageScale, SourceSizes};
use crate::activities::episode::StreamCatalog;
use crate::components::scoped_task::ScopedTask;
use crate::shell::notifications::MessageSource;
use crate::shell::state::StudioProjectState;

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct TrainingController {
    draft: TrainingDraftSignals,
    history: TrainingHistorySignals,
    commands: TrainingCommandSignals,
    source_sizes: SourceSizes,
}

#[derive(Clone, Copy, PartialEq)]
struct TrainingDraftSignals {
    config: Signal<TrainingConfig>,
    automatic_run_name: Signal<bool>,
    /// The image size as a fraction of the first camera's, which the size
    /// itself follows once that camera's is read.
    image_scale: Signal<ImageScale>,
}

#[derive(Clone, Copy, PartialEq)]
struct TrainingHistorySignals {
    selected_run: Signal<Option<String>>,
    opened_runs: Signal<Vec<String>>,
}

#[derive(Clone, Copy, PartialEq)]
struct TrainingCommandSignals {
    starting: Signal<bool>,
    stopping: Signal<Option<String>>,
    deleting: Signal<Option<String>>,
    /// The checkpoint being published, by run and step.
    publishing: Signal<Option<(String, u64)>>,
}

pub(crate) struct TrainingRunIdentity {
    id: String,
    name: String,
}

impl TrainingRunIdentity {
    pub(super) fn new(id: String, name: String) -> Self {
        Self { id, name }
    }
}

impl TrainingController {
    /// The draft trains on no view until one is chosen: its source names a
    /// view no catalog lists.
    pub(super) fn use_controller() -> Self {
        use_hook(|| Self {
            draft: TrainingDraftSignals {
                config: Signal::new(TrainingConfig::default()),
                automatic_run_name: Signal::new(true),
                image_scale: Signal::new(ImageScale::default()),
            },
            history: TrainingHistorySignals {
                selected_run: Signal::new(None),
                opened_runs: Signal::new(Vec::new()),
            },
            commands: TrainingCommandSignals {
                starting: Signal::new(false),
                stopping: Signal::new(None),
                deleting: Signal::new(None),
                publishing: Signal::new(None),
            },
            source_sizes: SourceSizes::new(),
        })
    }

    pub(super) fn config(&self) -> TrainingConfig {
        self.draft.config.read().to_owned()
    }

    pub(super) fn edit_config(mut self, edit: impl FnOnce(&mut TrainingConfig)) {
        edit(&mut self.draft.config.write());
    }

    /// Apply `edit` to the drafted ACT policy, when the draft trains one.
    pub(super) fn edit_act(mut self, edit: impl FnOnce(&mut ActModel)) {
        if let TrainingModel::Act(act) = self.draft.config.write().model_mut() {
            edit(act);
        }
    }

    /// Read `camera`, when the draft trains a progress model.
    pub(super) fn set_progress_camera(mut self, camera: String) {
        if let TrainingModel::Progress(progress) = self.draft.config.write().model_mut() {
            progress.set_camera(camera);
        }
    }

    /// Train a `kind` model with its family's defaults, its cameras chosen
    /// afresh and, unless the operator named the run, a name of its family
    /// among `existing_names`.
    pub(super) fn choose_model(mut self, kind: TrainingModelKind, existing_names: &[String]) {
        if self.draft.config.peek().model().kind() == kind {
            return;
        }
        let mut config = self.draft.config.write();
        config.set_model_kind(kind);
        if *self.draft.automatic_run_name.peek() {
            config.set_run_name(existing_names.next_default_run_name(kind));
        }
    }

    /// Train on `source`. Its cameras are chosen afresh once its dataset's
    /// are known.
    pub(super) fn choose_source(mut self, source: TrainingSource) {
        if *self.draft.config.peek().source() == source {
            return;
        }
        let mut config = self.draft.config.write();
        config.set_source(source);
        match config.model_mut() {
            TrainingModel::Act(act) => act.observation_mut().set_cameras(Vec::new()),
            TrainingModel::Progress(progress) => progress.set_camera(""),
        }
    }

    /// Choose among the dataset's `cameras` while the model reads none: a
    /// policy reads every one, a progress model the overview.
    pub(super) fn default_cameras(mut self, cameras: &[String]) {
        if !self.draft.config.peek().model().cameras().is_empty() || cameras.is_empty() {
            return;
        }
        match self.draft.config.write().model_mut() {
            TrainingModel::Act(act) => act.observation_mut().set_cameras(cameras.to_vec()),
            TrainingModel::Progress(progress) => {
                if let Some(overview) = StreamCatalog::overview_camera(cameras) {
                    progress.set_camera(overview);
                }
            }
        }
    }

    pub(super) fn image_scale(&self) -> ImageScale {
        *self.draft.image_scale.read()
    }

    pub(super) fn set_image_scale(mut self, scale: ImageScale) {
        self.draft.image_scale.set(scale);
    }

    /// Resize every frame a policy reads to `image`.
    pub(super) fn set_image(self, image: ImageSize) {
        let resized = match self.draft.config.peek().model() {
            TrainingModel::Act(act) => act.observation().image() != image,
            TrainingModel::Progress(_) => false,
        };
        if resized {
            self.edit_act(|act| act.observation_mut().set_image(image));
        }
    }

    /// The size each camera records at, as far as it has been read.
    pub(super) const fn source_sizes(&self) -> SourceSizes {
        self.source_sizes
    }

    pub(super) fn set_run_name(mut self, value: String) {
        self.draft.automatic_run_name.set(false);
        self.draft.config.write().set_run_name(value);
    }

    pub(super) fn sync_automatic_run_name(mut self, existing_names: &[String]) {
        if self.draft.automatic_run_name.peek().to_owned() {
            let kind = self.draft.config.peek().model().kind();
            self.draft
                .config
                .write()
                .set_run_name(existing_names.next_default_run_name(kind));
        }
    }

    pub(super) fn selected_run(&self) -> Option<String> {
        self.history.selected_run.read().to_owned()
    }

    pub(super) fn opened_runs(&self) -> Vec<String> {
        self.history.opened_runs.read().to_owned()
    }

    pub(super) fn starting(&self) -> bool {
        self.commands.starting.read().to_owned()
    }

    pub(super) fn stopping(&self) -> Option<String> {
        self.commands.stopping.read().to_owned()
    }

    pub(super) fn deleting(&self) -> Option<String> {
        self.commands.deleting.read().to_owned()
    }

    /// The checkpoint being published, by run and step.
    pub(super) fn publishing(&self) -> Option<(String, u64)> {
        self.commands.publishing.read().to_owned()
    }

    pub(super) fn open_run(mut self, id: String) {
        if !self.history.opened_runs.peek().contains(&id) {
            self.history.opened_runs.write().push(id.clone());
        }
        self.history.selected_run.set(Some(id));
    }

    pub(super) fn select_run(mut self, id: String) {
        self.history.selected_run.set(Some(id));
    }

    pub(super) fn close_run(mut self, id: String) {
        self.history
            .opened_runs
            .write()
            .retain(|opened| opened != &id);
        if self.history.selected_run.peek().as_deref() == Some(id.as_str()) {
            self.history.selected_run.set(None);
        }
    }

    pub(super) fn start(
        mut self,
        studio: StudioProjectState,
        history: super::history::TrainingHistory,
    ) {
        if self.commands.starting.peek().to_owned() {
            return;
        }
        self.commands.starting.set(true);
        let config = (self.draft.config)();
        let mut starting = self.commands.starting;
        let mut selected_run = self.history.selected_run;
        let mut opened_runs = self.history.opened_runs;
        let mut config_signal = self.draft.config;
        let mut automatic_run_name = self.draft.automatic_run_name;
        let training = history.snapshot();
        ScopedTask::spawn(self.draft.config.origin_scope(), async move {
            let result = TrainingClient::current()
                .start_training_run(config)
                .await
                .map_err(|error| error.to_string());
            starting.set(false);
            match result {
                Ok(run) => {
                    let mut existing_names = training
                        .read()
                        .runs()
                        .iter()
                        .map(|existing| existing.config().run_name().to_owned())
                        .collect::<Vec<_>>();
                    if !existing_names
                        .iter()
                        .any(|name| name == run.config().run_name())
                    {
                        existing_names.push(run.config().run_name().to_owned());
                    }
                    let kind = config_signal.peek().model().kind();
                    config_signal
                        .write()
                        .set_run_name(existing_names.next_default_run_name(kind));
                    automatic_run_name.set(true);
                    if !opened_runs.peek().iter().any(|id| id == run.id()) {
                        opened_runs.write().push(run.id().to_owned());
                    }
                    selected_run.set(Some(run.id().to_owned()));
                }
                Err(error) => studio.report_from(MessageSource::Training, error),
            }
        });
    }

    pub(super) fn stop(mut self, studio: StudioProjectState, id: String) {
        if self.commands.stopping.peek().is_some() {
            return;
        }
        self.commands.stopping.set(Some(id.clone()));
        let mut stopping = self.commands.stopping;
        studio.run_reported("Stop requested.", async move {
            let result = TrainingClient::current()
                .stop_training_run(id)
                .await
                .map_err(|error| error.to_string());
            stopping.set(None);
            result
        });
    }

    pub(super) fn delete(mut self, studio: StudioProjectState, run: TrainingRunIdentity) {
        if self.commands.deleting.peek().is_some() {
            return;
        }
        self.commands.deleting.set(Some(run.id.clone()));
        let deleted_id = run.id.clone();
        let run_id = run.id.clone();
        let mut deleting = self.commands.deleting;
        let mut selected_run = self.history.selected_run;
        let mut opened_runs = self.history.opened_runs;
        studio.run_reported(format!("Deleted run {}.", run.name), async move {
            let result = TrainingClient::current()
                .delete_training_run(run_id)
                .await
                .map_err(|error| error.to_string());
            deleting.set(None);
            if result.is_ok() {
                let selected = selected_run.peek().to_owned();
                let next_selected = opened_runs
                    .write()
                    .selection_after_close(selected.as_deref(), &deleted_id);
                if selected.as_deref() == Some(deleted_id.as_str()) {
                    selected_run.set(next_selected);
                }
            }
            result
        });
    }

    /// Publish the checkpoint `publication` names under its name; the
    /// checkpoint shows it once Training reads the published models again.
    pub(super) fn publish(mut self, studio: StudioProjectState, publication: ModelPublication) {
        if self.commands.publishing.peek().is_some() {
            return;
        }
        let checkpoint = (publication.run_id().to_owned(), publication.step());
        self.commands.publishing.set(Some(checkpoint));
        let mut publishing = self.commands.publishing;
        let name = publication.name().trim().to_owned();
        studio.run_reported(format!("Published “{name}”."), async move {
            let result = TrainingClient::current()
                .publish_model(publication)
                .await
                .map(drop)
                .map_err(|error| error.to_string());
            publishing.set(None);
            result
        });
    }
}

trait TrainingRunNames {
    /// The next unused default name of a `kind` run.
    fn next_default_run_name(&self, kind: TrainingModelKind) -> String;
}

impl TrainingRunNames for [String] {
    fn next_default_run_name(&self, kind: TrainingModelKind) -> String {
        let prefix = match kind {
            TrainingModelKind::Act => "act-run-",
            TrainingModelKind::Progress => "progress-run-",
        };
        let used = self.iter().map(String::as_str).collect::<BTreeSet<_>>();
        let mut number = self
            .iter()
            .filter_map(|name| name.strip_prefix(prefix)?.parse::<u64>().ok())
            .max()
            .unwrap_or_default()
            .saturating_add(1);
        loop {
            let candidate = format!("{prefix}{number:03}");
            if !used.contains(candidate.as_str()) {
                return candidate;
            }
            number = number.saturating_add(1);
        }
    }
}

trait OpenedTrainingRuns {
    fn selection_after_close(&mut self, selected: Option<&str>, closing: &str) -> Option<String>;
}

impl OpenedTrainingRuns for Vec<String> {
    fn selection_after_close(&mut self, selected: Option<&str>, closing: &str) -> Option<String> {
        let Some(index) = self.iter().position(|id| id == closing) else {
            return selected.map(str::to_owned);
        };
        self.remove(index);
        if selected != Some(closing) {
            return selected.map(str::to_owned);
        }
        self.get(index)
            .or_else(|| index.checked_sub(1).and_then(|index| self.get(index)))
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::{OpenedTrainingRuns, TrainingController, TrainingModelKind, TrainingRunNames};
    use dioxus::prelude::*;
    use std::{cell::RefCell, rc::Rc};

    #[derive(Clone, Default)]
    struct ControllerHarness(Rc<RefCell<Option<TrainingController>>>);

    impl ControllerHarness {
        fn render(self) -> Element {
            *self.0.borrow_mut() = Some(TrainingController::use_controller());
            rsx! {}
        }
    }

    /// Switching to a progress model takes its recipe and a name of its
    /// family, and the dataset's overview camera once its cameras are known.
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn a_progress_model_takes_its_family_defaults_and_the_overview_camera() {
        let harness = ControllerHarness::default();
        let mut dom = VirtualDom::new_with_props(ControllerHarness::render, harness.clone());
        dom.rebuild_in_place();
        let controller = harness.0.borrow().unwrap();
        dom.in_runtime(|| {
            controller.choose_model(TrainingModelKind::Progress, &["act-run-001".to_owned()]);
            let config = controller.config();
            assert_eq!(config.run_name(), "progress-run-001");
            assert_eq!((config.steps(), config.batch_size()), (15_000, 1_024));
            assert!(config.model().cameras().is_empty());
            controller
                .default_cameras(&["/left-wrist-camera".to_owned(), "/top-camera".to_owned()]);
            assert_eq!(controller.config().model().cameras(), ["/top-camera"]);
            controller.choose_model(TrainingModelKind::Act, &[]);
            assert_eq!(controller.config().batch_size(), 8);
            controller
                .default_cameras(&["/left-wrist-camera".to_owned(), "/top-camera".to_owned()]);
            assert_eq!(controller.config().model().cameras().len(), 2);
        });
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn default_run_names_advance_from_history_without_colliding() {
        let names = [
            "act-run-001".to_owned(),
            "experiment".to_owned(),
            "act-run-007".to_owned(),
            "act-run-008".to_owned(),
        ];
        assert_eq!(
            names.next_default_run_name(TrainingModelKind::Act),
            "act-run-009"
        );
        assert_eq!(
            names.next_default_run_name(TrainingModelKind::Progress),
            "progress-run-001"
        );
        assert_eq!(
            Vec::<String>::new().next_default_run_name(TrainingModelKind::Act),
            "act-run-001"
        );
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn closing_selected_run_prefers_the_tab_to_its_right() {
        let mut opened = vec!["a".to_owned(), "b".to_owned(), "c".to_owned()];
        let selected = opened.selection_after_close(Some("b"), "b");
        assert_eq!(opened, ["a", "c"]);
        assert_eq!(selected.as_deref(), Some("c"));
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn closing_last_selected_run_falls_back_left_then_to_empty() {
        let mut opened = vec!["a".to_owned(), "b".to_owned()];
        let selected = opened.selection_after_close(Some("b"), "b");
        assert_eq!(selected.as_deref(), Some("a"));
        let selected = opened.selection_after_close(selected.as_deref(), "a");
        assert!(opened.is_empty());
        assert_eq!(selected, None);
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn closing_background_run_preserves_selection() {
        let mut opened = vec!["a".to_owned(), "b".to_owned(), "c".to_owned()];
        let selected = opened.selection_after_close(Some("c"), "a");
        assert_eq!(opened, ["b", "c"]);
        assert_eq!(selected.as_deref(), Some("c"));
    }
}
