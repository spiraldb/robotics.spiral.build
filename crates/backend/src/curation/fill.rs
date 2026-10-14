//! Fill runs: ask each model about the episodes that need its answers, write
//! them as columns of the dataset's answers table, and then publish the
//! views that read them.
//!
//! A run works through its columns in the order the browser compiled them,
//! so every column is filled before a later one reads it to find the
//! episodes that reach its node. For each, it asks about the episodes the
//! column does not hold yet, a few at a time, and holds the answers in
//! memory until they fill one write, which is written while it asks on; a
//! failed answer is written too, so it is never asked again. Once every
//! column is written it creates or replaces its views. Stop starts nothing
//! more; what is in flight lands and is written. An error does the same,
//! then ends the run. A run lives in memory: a restarted backend loses the
//! answers held since the last write, and publishing again resumes past
//! every episode already written.
use super::break_down::EpisodeBreakdown;
use super::object_tracking::ObjectTrackingService;
use super::openrouter::OpenRouter;
use super::progress_service::ProgressService;
use super::subtask_judgment::{
    CategoryQuestions, SimilarityQuestions, StoredSubtasks, SubtaskJudgment,
};
use super::video_service::{VideoAnswer, VideoQuestion};
use crate::OperationError;
use crate::held_runs::{ABANDONED_AFTER, HeldRuns, HeldWork};
use crate::sync::MutexRecovery;
use arrow_array::RecordBatch;
use arrow_array::cast::AsArray as _;
use arrow_schema::DataType;
use futures_util::StreamExt as _;
use spiral_robotics_common::curation::{
    AnswerColumn, AnswerSource, AnswerTable, ColumnFill, EpisodeFailure, FillEpisodes, FillRequest,
    FillState, FillStatus, FillStep, ViewName,
};
use spiral_robotics_common::datasets::{
    CameraVideo, DatasetInspection, DatasetSelection, DatasetStreams, EpisodeId, EpisodeSelection,
    EpisodeTask,
};
use spiral_robotics_common::relations::EpisodeQueryRequest;
use spiral_robotics_common::sql::{ReadOnlyQuery, ReadOnlyQueryMode};
use spiral_robotics_storage::answers::{
    ColumnAnswers, DatasetAnswers, EpisodeAnswer, StatementAnswers,
};
use spiral_robotics_storage::{FfmpegExecutable, ProviderKeys, StorageManager};
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use tokio::sync::{OnceCell, Semaphore, mpsc};
use tokio::time::Instant;

/// How many episodes a column asks about at once when no Modal service's
/// capacity bounds it: as many as each Spiral statement asking a model
/// about one episode had in flight.
const EPISODES_IN_FLIGHT: usize = 8;
/// How many requests the progress service has in flight, across every run.
/// The uploads bound it: each container takes in eight at once while its GPU
/// scores (`target_inputs` in `modal/warp_progress/app.py`), and more in
/// flight only splits the upload bandwidth.
const SCORINGS_IN_FLIGHT: usize = 20;
/// How many requests the object tracking service has in flight, across every
/// run: each of its H100 containers takes four at once while its two model
/// workers track (`target_inputs` in `modal/object_tracking/app.py`), so
/// this keeps two containers busy.
const TRACKINGS_IN_FLIGHT: usize = 8;
/// What a run reports while it creates its views.
const PUBLISHING: &str = "Publishing";

/// The fill runs one project has started.
pub(crate) struct FillRuns {
    storage: Arc<StorageManager>,
    keys: Arc<ProviderKeys>,
    ffmpeg: FfmpegExecutable,
    scorings: Arc<Semaphore>,
    trackings: Arc<Semaphore>,
    runs: HeldRuns<FillRun>,
}

impl FillRuns {
    pub(crate) fn new(storage: Arc<StorageManager>, keys: Arc<ProviderKeys>) -> Self {
        Self {
            storage,
            keys,
            ffmpeg: FfmpegExecutable::from_environment(),
            scorings: Arc::new(Semaphore::new(SCORINGS_IN_FLIGHT)),
            trackings: Arc::new(Semaphore::new(TRACKINGS_IN_FLIGHT)),
            runs: HeldRuns::new("fill"),
        }
    }

    /// `request`, admitted as a run: every statement read-only, every view
    /// named as a view, every judgment's questions ones jev takes, and each
    /// Modal service and OpenRouter configured when a column needs it.
    pub(super) fn admit(&self, request: FillRequest) -> Result<AdmittedFill, OperationError> {
        let invalid = OperationError::InvalidInput;
        if request.columns().is_empty() && request.views().is_empty() {
            return Err(invalid("A fill run needs a column or a view.".into()));
        }
        let mut progress = None;
        let mut tracking = None;
        let mut openrouter = None;
        for column in request.columns() {
            if let FillEpisodes::Reaching(statement) = column.episodes() {
                Self::read_only(statement)?;
            }
            match column.source() {
                AnswerSource::DatasetStatement { statement } => Self::read_only(statement)?,
                AnswerSource::ProgressService { .. } if progress.is_none() => {
                    progress = Some(
                        ProgressService::of(&self.keys, self.scorings.clone()).map_err(invalid)?,
                    );
                }
                AnswerSource::ObjectTracking { .. } if tracking.is_none() => {
                    tracking = Some(
                        ObjectTrackingService::of(&self.keys, self.trackings.clone())
                            .map_err(invalid)?,
                    );
                }
                AnswerSource::Similarity(call) => {
                    SimilarityQuestions::new(call.query()).map_err(invalid)?;
                }
                AnswerSource::Classify(call) => {
                    CategoryQuestions::new(call.categories()).map_err(invalid)?;
                }
                AnswerSource::ProgressService { .. }
                | AnswerSource::ObjectTracking { .. }
                | AnswerSource::BreakDown(_) => {}
            }
            let asks_openrouter = matches!(
                column.source(),
                AnswerSource::BreakDown(_)
                    | AnswerSource::Similarity(_)
                    | AnswerSource::Classify(_)
            );
            if asks_openrouter && openrouter.is_none() {
                openrouter = Some(OpenRouter::of(&self.keys, column.node()).map_err(invalid)?);
            }
        }
        let mut views = Vec::new();
        for view in request.views() {
            views.push(ViewName::try_from(view.name()).map_err(invalid)?);
            Self::read_only(view.statement())?;
        }
        Ok(AdmittedFill {
            views,
            run: FillRun::new(request),
            work: FillWork {
                storage: self.storage.clone(),
                progress,
                tracking,
                openrouter,
                ffmpeg: self.ffmpeg.clone(),
                inspection: OnceCell::new(),
            },
        })
    }

    /// Start `admitted`, replacing any run that publishes one of its views,
    /// and say where the new run stands.
    pub(super) fn start(&self, admitted: AdmittedFill) -> FillStatus {
        let AdmittedFill { views, run, work } = admitted;
        let publishes = views
            .iter()
            .map(|view| view.as_str().to_owned())
            .collect::<BTreeSet<_>>();
        if !publishes.is_empty() {
            self.runs.replace(|run| run.publishes_any(&publishes));
        }
        let run = Arc::new(run);
        let id = self.runs.start(run.clone(), run.clone().fill(work));
        run.status(&id)
    }

    /// Where run `id` stands. Reading it keeps a preview's run from being
    /// forgotten.
    pub(super) fn status(&self, id: &str) -> Result<FillStatus, OperationError> {
        self.runs.forget();
        let run = self.runs.get(id)?;
        run.polled();
        Ok(run.status(id))
    }

    /// Every run that publishes a view, as Data lists them.
    pub(super) fn publications(&self) -> Vec<FillStatus> {
        self.runs
            .all()
            .into_iter()
            .filter(|(_, run)| !run.request.views().is_empty())
            .map(|(id, run)| run.status(&id))
            .collect()
    }

    /// Ask run `id` to start nothing more.
    pub(super) fn stop(&self, id: &str) -> Result<(), OperationError> {
        self.runs.get(id)?.stop();
        Ok(())
    }

    /// End every run now: the project is closing.
    pub(super) fn shutdown(&self) {
        self.runs.shutdown();
    }

    fn read_only(statement: &str) -> Result<(), OperationError> {
        ReadOnlyQuery::new(statement, ReadOnlyQueryMode::Curation)
            .authorize()
            .map_err(|error| OperationError::InvalidInput(error.to_string()))
    }
}

/// A run admitted but not started: the views it publishes, and what it
/// works with.
pub(super) struct AdmittedFill {
    views: Vec<ViewName>,
    run: FillRun,
    work: FillWork,
}

/// What a run works with: the project's storage, each Modal service and
/// OpenRouter when one needs it, the FFmpeg Break down decodes with, and
/// the dataset's streams once a camera's video or an instruction is asked
/// about.
struct FillWork {
    storage: Arc<StorageManager>,
    progress: Option<ProgressService>,
    tracking: Option<ObjectTrackingService>,
    openrouter: Option<OpenRouter>,
    ffmpeg: FfmpegExecutable,
    /// The dataset as the run first read it for a topic: the streams the
    /// browser's camera and instruction topics are found in.
    inspection: OnceCell<DatasetInspection>,
}

/// One episode's answer, and why it failed when it did.
pub(super) struct Answered {
    answer: EpisodeAnswer,
    failure: Option<String>,
}

impl Answered {
    pub(super) fn given(answer: EpisodeAnswer) -> Self {
        Self {
            answer,
            failure: None,
        }
    }

    /// No answer of `answer_type`, for `reason`.
    pub(super) fn failed(answer_type: &DataType, reason: impl Into<String>) -> Self {
        Self {
            answer: EpisodeAnswer::failed(answer_type),
            failure: Some(reason.into()),
        }
    }

    pub(super) fn into_parts(self) -> (EpisodeAnswer, Option<String>) {
        (self.answer, self.failure)
    }
}

/// The answers a column's asking holds until they fill one write, each full
/// set handed to the writer, and how many it has answered.
struct HeldAnswers {
    held: ColumnAnswers,
    answered: usize,
    writer: mpsc::UnboundedSender<ColumnAnswers>,
}

impl HeldAnswers {
    fn new(writer: mpsc::UnboundedSender<ColumnAnswers>) -> Self {
        Self {
            held: ColumnAnswers::default(),
            answered: 0,
            writer,
        }
    }

    /// Hold `episode`'s answer, and hand what is held to the writer once it
    /// fills one write.
    fn hold(&mut self, episode: EpisodeId, answer: EpisodeAnswer) {
        self.held.hold(episode, answer);
        self.answered += 1;
        if self.held.is_full() {
            self.hand_over();
        }
    }

    /// How many episodes it has answered, written or not.
    fn answered(&self) -> usize {
        self.answered
    }

    /// Hand the rest to the writer, and let go of it, so it stops once that
    /// is written.
    fn finish(mut self) {
        self.hand_over();
    }

    fn hand_over(&mut self) {
        let set = std::mem::take(&mut self.held);
        // The writer takes sets until every sender is gone, so it is there.
        let _ = self.writer.send(set);
    }
}

impl FillWork {
    /// How many of a column's episodes `source` is asked about at once.
    fn in_flight(source: &AnswerSource) -> usize {
        match source {
            AnswerSource::DatasetStatement { .. }
            | AnswerSource::BreakDown(_)
            | AnswerSource::Similarity(_)
            | AnswerSource::Classify(_) => EPISODES_IN_FLIGHT,
            AnswerSource::ProgressService { .. } => SCORINGS_IN_FLIGHT,
            AnswerSource::ObjectTracking { .. } => TRACKINGS_IN_FLIGHT,
        }
    }

    /// Ask `source` about `episode`. An `Err` is the run's: the episode's
    /// answer is neither given nor failed, and it is asked again next run.
    async fn answer(
        &self,
        request: &FillRequest,
        source: &AnswerSource,
        episode: &EpisodeId,
    ) -> Result<Answered, String> {
        let selected = EpisodeSelection::new(request.dataset().clone(), episode.clone());
        match source {
            AnswerSource::DatasetStatement { .. } => Err(
                "A dataset statement answers every episode in one read, not one at a time.".into(),
            ),
            AnswerSource::ProgressService {
                camera,
                model,
                revision,
            } => {
                let service = self
                    .progress
                    .as_ref()
                    .ok_or("The progress service is not set up.")?;
                self.video_answer(selected, camera, service.scoring(model, revision))
                    .await
            }
            AnswerSource::ObjectTracking {
                camera,
                prompt,
                rate,
                model,
                revision,
            } => {
                let service = self
                    .tracking
                    .as_ref()
                    .ok_or("The object tracking service is not set up.")?;
                let tracking = service.tracking((prompt, *rate), (model, revision));
                self.video_answer(selected, camera, tracking).await
            }
            AnswerSource::BreakDown(call) => {
                let breakdown = EpisodeBreakdown::new(call, self.openrouter()?, &self.ffmpeg);
                let Some(packets) = self.camera_packets(&selected, call.camera()).await? else {
                    let reason = Self::unrecorded(call.camera());
                    return Ok(Answered::failed(&EpisodeBreakdown::answer_type(), reason));
                };
                let task = match call.task() {
                    Some(task) => self.episode_task(selected, task).await?,
                    None => None,
                };
                breakdown.answer(&packets, task.as_deref()).await
            }
            AnswerSource::Similarity(call) => {
                let questions = SimilarityQuestions::new(call.query())?;
                let judgment = SubtaskJudgment::new(questions, call.model(), self.openrouter()?);
                let stored = self.stored_subtasks(selected, call.subtasks()).await?;
                judgment.answer(episode, stored).await
            }
            AnswerSource::Classify(call) => {
                let questions = CategoryQuestions::new(call.categories())?;
                let judgment = SubtaskJudgment::new(questions, call.model(), self.openrouter()?);
                let stored = self.stored_subtasks(selected, call.subtasks()).await?;
                judgment.answer(episode, stored).await
            }
        }
    }

    fn openrouter(&self) -> Result<&OpenRouter, String> {
        self.openrouter
            .as_ref()
            .ok_or_else(|| "OpenRouter is not set up.".to_owned())
    }

    /// What `question` answers about the `selected` episode's video on
    /// `camera`, or the episode's failed answer when that camera recorded no
    /// video of it.
    async fn video_answer(
        &self,
        selected: EpisodeSelection,
        camera: &str,
        question: VideoQuestion<'_>,
    ) -> Result<Answered, String> {
        let answered = match self.camera_packets(&selected, camera).await? {
            Some(packets) => question.ask(&packets).await?,
            None => VideoAnswer::Rejected(Self::unrecorded(camera)),
        };
        Ok(match answered {
            VideoAnswer::Answered(answer) => Answered::given(answer),
            VideoAnswer::Rejected(reason) => Answered::failed(&question.answer_type(), reason),
        })
    }

    /// The `selected` episode's packets on the camera topic `camera`, in
    /// decode order, as [`CameraVideo::packets`] reads them; none when that
    /// camera recorded no video of it.
    async fn camera_packets(
        &self,
        selected: &EpisodeSelection,
        camera: &str,
    ) -> Result<Option<RecordBatch>, String> {
        let streams = self.streams(selected.dataset()).await?;
        let stream = streams.camera(camera)?.revision();
        let video = CameraVideo::new(selected.dataset(), stream).packets();
        let bound = EpisodeQueryRequest::new(selected.clone(), video, 1);
        let (schema, batches) = self.storage.queries().batches_for_episode(&bound).await?;
        let packets = arrow_select::concat::concat_batches(&schema, &batches)
            .map_err(|error| error.to_string())?;
        Ok((packets.num_rows() > 0).then_some(packets))
    }

    fn unrecorded(camera: &str) -> String {
        format!("This episode records no {camera} video.")
    }

    /// The `selected` episode's task, where `task` says its dataset records
    /// it: the first message it records on the instruction topic, or the
    /// first task its dataset lists beside it; none when it records none.
    async fn episode_task(
        &self,
        selected: EpisodeSelection,
        task: &EpisodeTask,
    ) -> Result<Option<String>, String> {
        let dataset = selected.dataset();
        let statement = match task {
            EpisodeTask::Recorded(topic) => {
                let streams = self.streams(dataset).await?;
                format!(
                    "SELECT r.value.data AS task FROM {} r WHERE r.id = $1 \
                     ORDER BY r.log_time, r.ordinal LIMIT 1",
                    dataset.messages(streams.topic(topic)?.revision())
                )
            }
            EpisodeTask::Listed => format!(
                "SELECT spql.list.get(e.tasks, 0) AS task FROM {} e WHERE e.id = $1",
                dataset.episodes()
            ),
        };
        let bound = EpisodeQueryRequest::new(selected, statement, 1);
        let (_, batches) = self.storage.queries().batches_for_episode(&bound).await?;
        let Some(batch) = batches.iter().find(|batch| batch.num_rows() > 0) else {
            return Ok(None);
        };
        let tasks = batch
            .column_by_name("task")
            .ok_or("The task was not read.")?;
        let tasks = arrow_cast::cast(tasks, &DataType::Utf8).map_err(|error| error.to_string())?;
        Ok(tasks
            .as_string::<i32>()
            .iter()
            .next()
            .flatten()
            .map(str::to_owned))
    }

    /// The subtasks the Break down column `column` stores for the `selected`
    /// episode.
    async fn stored_subtasks(
        &self,
        selected: EpisodeSelection,
        column: &AnswerColumn,
    ) -> Result<StoredSubtasks, String> {
        let statement = StoredSubtasks::statement(selected.dataset(), column);
        let bound = EpisodeQueryRequest::new(selected, statement, 1);
        let (_, batches) = self.storage.queries().batches_for_episode(&bound).await?;
        StoredSubtasks::read(&batches)
    }

    /// The streams `dataset` records, among which the browser's topics are
    /// found, which a run reads once.
    async fn streams(&self, dataset: &DatasetSelection) -> Result<DatasetStreams<'_>, String> {
        let inspection = self
            .inspection
            .get_or_try_init(|| self.storage.inspect_dataset(dataset.clone()))
            .await?;
        Ok(DatasetStreams::new(inspection.topics()))
    }
}

/// One run: what it was asked, and how far it has got.
struct FillRun {
    request: FillRequest,
    progress: Mutex<FillProgress>,
}

/// The part of a run its task and its polls both change.
struct FillProgress {
    state: FillState,
    failures: Vec<EpisodeFailure>,
    halted: bool,
    polled: Instant,
}

impl FillRun {
    fn new(request: FillRequest) -> Self {
        let first = request
            .columns()
            .first()
            .map_or(PUBLISHING, ColumnFill::node)
            .to_owned();
        Self {
            request,
            progress: Mutex::new(FillProgress {
                state: FillState::Running(FillStep::new(first, 0, None)),
                failures: Vec::new(),
                halted: false,
                polled: Instant::now(),
            }),
        }
    }

    /// Fill every column in order, then publish every view.
    async fn fill(self: Arc<Self>, work: FillWork) {
        let outcome = async {
            for column in self.request.columns() {
                self.fill_column(column, &work).await?;
                if self.progress.recover_lock().halted {
                    return Ok(FillState::Stopped);
                }
            }
            if !self.request.views().is_empty() {
                self.report(FillStep::new(PUBLISHING, 0, None));
            }
            let views = work.storage.curated_views();
            for view in self.request.views() {
                let name = ViewName::try_from(view.name())?;
                views.replace(&name, view.statement()).await?;
            }
            Ok::<_, String>(FillState::Finished)
        }
        .await;
        self.progress.recover_lock().state = match outcome {
            Ok(state) => state,
            Err(reason) => FillState::Failed(reason),
        };
    }

    /// Answer every episode `column` is filled for that it does not hold yet,
    /// and write what was answered, even when the run stops or fails part
    /// way. Each write waits for every Spiral read of the project to end, so
    /// it runs beside the asking, which keeps the reads in flight moving.
    async fn fill_column(&self, column: &ColumnFill, work: &FillWork) -> Result<(), String> {
        let node = column.node();
        self.report(FillStep::new(node, 0, None));
        let listed = match column.episodes() {
            FillEpisodes::Reaching(statement) => {
                work.storage.queries().episode_ids(statement).await?
            }
            FillEpisodes::Listed(episodes) => episodes.clone(),
        };
        let answers = work
            .storage
            .dataset_answers(AnswerTable::of(self.request.dataset()));
        let mut skipped = answers.filled(column.column()).await?;
        let needed = listed
            .into_iter()
            .filter(|episode| skipped.insert(episode.clone()))
            .collect::<Vec<_>>();
        let total = needed.len();
        self.report(FillStep::new(node, 0, Some(total)));
        let (writer, sets) = mpsc::unbounded_channel();
        let asking = async {
            let mut held = HeldAnswers::new(writer);
            let failed = match column.source() {
                AnswerSource::DatasetStatement { statement } => self
                    .answer_at_once(node, statement, needed, work, &mut held)
                    .await
                    .err(),
                source => {
                    self.answer_each(node, source, needed, work, &mut held)
                        .await
                }
            };
            held.finish();
            failed
        };
        let (failed, unwritten) =
            tokio::join!(asking, self.write_each(&answers, column.column(), sets));
        unwritten.or(failed).map_or(Ok(()), Err)
    }

    /// Write each set of answers handed over, in order, until the asking
    /// lets go; the first write that failed, which halts the run.
    async fn write_each(
        &self,
        answers: &DatasetAnswers<'_>,
        column: &AnswerColumn,
        mut sets: mpsc::UnboundedReceiver<ColumnAnswers>,
    ) -> Option<String> {
        let mut failed = None::<String>;
        while let Some(set) = sets.recv().await {
            if let Err(reason) = answers.write(column, set).await {
                self.progress.recover_lock().halted = true;
                failed.get_or_insert(reason);
            }
        }
        failed
    }

    /// Answer `needed` with one read of `statement` over the whole dataset.
    /// An episode it measured nothing for is answered as failed, so it is
    /// not asked again.
    async fn answer_at_once(
        &self,
        node: &str,
        statement: &str,
        needed: Vec<EpisodeId>,
        work: &FillWork,
        held: &mut HeldAnswers,
    ) -> Result<(), String> {
        if needed.is_empty() {
            return Ok(());
        }
        let total = needed.len();
        let (schema, batches) = work.storage.queries().batches(statement).await?;
        let mut answered = StatementAnswers::from_statement(&schema, &batches)?;
        for episode in needed {
            let answer = answered.answer(&episode);
            held.hold(episode, answer);
        }
        self.report(FillStep::new(node, held.answered(), Some(total)));
        Ok(())
    }

    /// Ask `source` about each of `needed`, a few at a time, holding every
    /// answer; why the run must end, if it must.
    async fn answer_each(
        &self,
        node: &str,
        source: &AnswerSource,
        needed: Vec<EpisodeId>,
        work: &FillWork,
        held: &mut HeldAnswers,
    ) -> Option<String> {
        let total = needed.len();
        let mut failed = None::<String>;
        let mut answering = futures_util::stream::iter(needed)
            .take_while(|_| std::future::ready(!self.progress.recover_lock().halted))
            .map(|episode| async move {
                let answer = work.answer(&self.request, source, &episode).await;
                (episode, answer)
            })
            .buffer_unordered(FillWork::in_flight(source));
        while let Some((episode, answer)) = answering.next().await {
            match answer.map(Answered::into_parts) {
                Ok((answer, failure)) => {
                    let mut progress = self.progress.recover_lock();
                    if let Some(reason) = failure {
                        progress
                            .failures
                            .push(EpisodeFailure::new(episode.clone(), reason));
                    }
                    held.hold(episode, answer);
                    progress.state =
                        FillState::Running(FillStep::new(node, held.answered(), Some(total)));
                }
                Err(reason) => {
                    self.progress.recover_lock().halted = true;
                    failed.get_or_insert(reason);
                }
            }
        }
        drop(answering);
        failed
    }

    fn report(&self, step: FillStep) {
        self.progress.recover_lock().state = FillState::Running(step);
    }

    fn status(&self, id: &str) -> FillStatus {
        let progress = self.progress.recover_lock();
        let views = self
            .request
            .views()
            .iter()
            .map(|view| view.name().to_owned())
            .collect();
        FillStatus::new(
            id.to_owned(),
            views,
            progress.state.clone(),
            progress.failures.clone(),
        )
    }

    fn polled(&self) {
        self.progress.recover_lock().polled = Instant::now();
    }

    /// Start nothing more; what is in flight lands and is written.
    fn stop(&self) {
        self.progress.recover_lock().halted = true;
    }

    fn publishes_any(&self, views: &BTreeSet<String>) -> bool {
        self.request
            .views()
            .iter()
            .any(|view| views.contains(view.name()))
    }
}

/// A preview's run is dropped once it settles and nobody has asked about it
/// for a while; a publishing run stays, so Data can say how it ended, until
/// the view is published again.
impl HeldWork for FillRun {
    fn is_forgotten(&self) -> bool {
        let progress = self.progress.recover_lock();
        self.request.views().is_empty()
            && progress.state.is_settled()
            && progress.polled.elapsed() > ABANDONED_AFTER
    }
}

#[cfg(test)]
mod tests;
