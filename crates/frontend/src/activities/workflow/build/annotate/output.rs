//! What one annotation node produced: the index of intervals it wrote, and
//! the episode one of them belongs to, playing beside it.
use super::episodes::AnnotationEpisode;
use crate::activities::data::{EpisodeReview, EpisodeTransport};
use crate::activities::workflow::build::document::SubtaskRow;
use crate::activities::workflow::build::poster::PosterFrames;
use crate::activities::workflow::build::review::{
    INTERVAL_MARGIN_SECONDS, SubtaskContext, SubtaskEntry, SubtaskStanding,
};
use crate::components::timeline::{IntervalTrack, TimelineInterval};
use dioxus::prelude::*;

/// One episode's rows, in time order.
#[derive(Clone, PartialEq)]
pub(super) struct EpisodeSubtasks {
    episode: String,
    rows: Vec<SubtaskRow>,
}

impl EpisodeSubtasks {
    /// Group an annotation node's rows by episode, keeping the order the
    /// document stores them in.
    pub(super) fn group(rows: impl Iterator<Item = SubtaskRow>) -> Vec<Self> {
        let mut grouped = Vec::<Self>::new();
        for row in rows {
            match grouped
                .iter_mut()
                .find(|group| group.episode == row.episode())
            {
                Some(group) => group.rows.push(row),
                None => grouped.push(Self {
                    episode: row.episode().to_owned(),
                    rows: vec![row],
                }),
            }
        }
        grouped
    }
    /// An episode the node has no rows for, so the player still opens.
    pub(super) fn none(episode: &str) -> Self {
        Self {
            episode: episode.to_owned(),
            rows: Vec::new(),
        }
    }
    pub(super) fn episode(&self) -> &str {
        &self.episode
    }
    pub(super) fn rows(&self) -> &[SubtaskRow] {
        &self.rows
    }
    /// The subtasks as one labeled timeline row.
    fn track(&self) -> IntervalTrack {
        IntervalTrack::new(
            "build-annotate-subtasks",
            "Subtasks",
            self.rows
                .iter()
                .map(|row| {
                    TimelineInterval::labeled(row.start_time(), row.end_time(), row.description())
                })
                .collect(),
        )
    }
}

/// Which interval the operator is looking at: the episode its player shows,
/// and the row inside it, if they picked one.
///
/// Both halves of the review read this, so the index and the recording can
/// never disagree about what is on screen.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct ChosenSubtask {
    episode: Signal<Option<String>>,
    /// The chosen row's start time, which is what identifies a row within its
    /// episode everywhere else in Build.
    start: Signal<Option<f64>>,
    /// The interval the player has been asked to play but has not yet.
    play: Signal<Option<(f64, f64)>>,
}

impl ChosenSubtask {
    pub(super) fn use_chosen() -> Self {
        Self {
            episode: use_signal(|| None),
            start: use_signal(|| None),
            play: use_signal(|| None),
        }
    }
    pub(super) fn episode(&self) -> Option<String> {
        self.episode.read().clone()
    }
    /// Show one whole episode without playing any particular interval, as the
    /// index does when its heading is pressed.
    pub(super) fn show(mut self, episode: &str) {
        self.episode.set(Some(episode.to_owned()));
        self.start.set(None);
    }
    /// Show one row and play it.
    pub(super) fn play_row(mut self, row: &SubtaskRow) {
        self.episode.set(Some(row.episode().to_owned()));
        self.start.set(Some(row.start_time()));
        self.play.set(Some((row.start_time(), row.end_time())));
    }
    /// Show an episode and, when an exploration named one, its interval.
    pub(super) fn focus(mut self, episode: &str, interval: Option<(f64, f64)>) {
        self.episode.set(Some(episode.to_owned()));
        self.start.set(interval.map(|(start, _)| start));
        self.play.set(interval);
    }
    fn holds(&self, row: &SubtaskRow) -> bool {
        self.episode.read().as_deref() == Some(row.episode())
            && *self.start.read() == Some(row.start_time())
    }
    /// Take the interval waiting to be played, if one is.
    fn take_play(mut self) -> Option<(f64, f64)> {
        self.play.write().take()
    }
}

/// Every annotated interval, grouped under the episode it came from.
#[component]
pub(super) fn SubtaskIndex(
    groups: Vec<EpisodeSubtasks>,
    context: SubtaskContext,
    frames: PosterFrames,
    chosen: ChosenSubtask,
) -> Element {
    if groups.is_empty() {
        return rsx! {
            p { class: "build-annotate-empty", role: "status",
                "No subtasks yet. Run the annotation to fill this list." }
        };
    }
    rsx! {
        ol { class: "build-entries build-entries-grouped", "aria-label": "Annotated subtasks by episode",
            for group in groups {
                li { key: "{group.episode}", class: "build-entries-group",
                    "aria-current": (chosen.episode().as_deref() == Some(group.episode())).then_some("true"),
                    button {
                        r#type: "button",
                        class: "build-entries-heading",
                        title: "{group.episode}",
                        onclick: {
                            let episode = group.episode.clone();
                            move |_| chosen.show(&episode)
                        },
                        span { class: "build-entries-heading-name", "{context.name(group.episode())}" }
                        span { class: "build-entries-heading-count",
                            "{Subtasks(group.rows().len())}" }
                    }
                    ol { class: "build-entries",
                        for row in group.rows() {
                            SubtaskEntry {
                                key: "{row.start_time()}",
                                row: row.clone(),
                                standing: SubtaskStanding::plain(chosen.holds(row)),
                                frames,
                                on_select: {
                                    let row = row.clone();
                                    move |()| chosen.play_row(&row)
                                },
                            }
                        }
                    }
                }
            }
        }
    }
}

/// `3 subtasks`, and `1 subtask` when that is what there is.
#[allow(non_snake_case)]
fn Subtasks(count: usize) -> String {
    if count == 1 {
        "1 subtask".to_owned()
    } else {
        format!("{count} subtasks")
    }
}

/// The chosen episode on the camera the run sampled, with its subtasks on the
/// shared timeline.
#[component]
pub(super) fn AnnotatedEpisode(
    episode: AnnotationEpisode,
    subtasks: EpisodeSubtasks,
    context: SubtaskContext,
    chosen: ChosenSubtask,
) -> Element {
    let transport = EpisodeTransport::use_opened_episode(episode.opened());
    let track = subtasks.track();
    let requested = transport.clone();
    use_effect(move || {
        if let Some((start, end)) = chosen.take_play() {
            requested.clone().play_interval(
                (start - INTERVAL_MARGIN_SECONDS).max(0.0),
                end + INTERVAL_MARGIN_SECONDS,
            );
        }
    });
    let activated = transport.clone();
    rsx! {
        header { class: "build-review-head",
            p { class: "build-review-subject", "{context.name(subtasks.episode())}" }
            p { class: "build-review-facts",
                span { title: "{subtasks.episode()}", "{Subtasks(subtasks.rows().len())}" }
                if let Some(camera) = context.camera() {
                    span { "{camera}" }
                }
            }
        }
        section { class: "build-review-player", "aria-label": "Annotated episode",
            EpisodeReview {
                episode: episode.opened().clone(),
                transport,
                span_tracks: if track.is_empty() { Vec::new() } else { vec![track] },
                shown_cameras: context.camera().map(str::to_owned).into_iter().collect::<Vec<_>>(),
                on_interval: move |interval: TimelineInterval| {
                    let (start, end) = interval.span();
                    activated.clone().play_interval(
                        (start - INTERVAL_MARGIN_SECONDS).max(0.0),
                        end + INTERVAL_MARGIN_SECONDS,
                    );
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(episode: &str, start: f64, end: f64) -> SubtaskRow {
        SubtaskRow::new("break_down_subtasks-1", episode, start, end, "fold").expect("a valid row")
    }

    #[test]
    fn rows_group_by_episode_in_the_order_the_document_stores_them() {
        let groups = EpisodeSubtasks::group(
            [
                row("ep-1", 0.0, 8.0),
                row("ep-1", 8.0, 13.0),
                row("ep-2", 0.0, 4.0),
            ]
            .into_iter(),
        );
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].episode(), "ep-1");
        assert_eq!(groups[0].rows().len(), 2);
        assert_eq!(groups[1].rows()[0].start_time(), 0.0);
        assert!(!groups[0].track().is_empty());
        assert!(EpisodeSubtasks::group(std::iter::empty()).is_empty());
    }

    #[test]
    fn a_count_names_what_it_counts_in_the_singular_too() {
        assert_eq!(Subtasks(1), "1 subtask");
        assert_eq!(Subtasks(0), "0 subtasks");
        assert_eq!(Subtasks(12), "12 subtasks");
    }
}
