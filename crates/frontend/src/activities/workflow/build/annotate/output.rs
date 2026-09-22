//! What one annotation node produced: the subtask table, and the selected
//! episode's recording with those subtasks on its timeline.
use super::episodes::AnnotationEpisode;
use crate::activities::data::{EpisodeReview, EpisodeTransport};
use crate::activities::workflow::build::document::SubtaskRow;
use crate::components::timeline::{IntervalTrack, TimelineInterval};
use crate::format::ClockPresentation as _;
use dioxus::prelude::*;

/// A row activated from the table plays with a little lead-in and run-out, so
/// the operator sees the motion enter and leave the interval.
const INTERVAL_MARGIN_SECONDS: f64 = 0.5;

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

/// The table of every annotated interval, grouped by episode.
#[component]
pub(super) fn SubtaskTable(
    groups: Vec<EpisodeSubtasks>,
    selected: Signal<Option<String>>,
    play: Signal<Option<(f64, f64)>>,
) -> Element {
    if groups.is_empty() {
        return rsx! {
            p { class: "build-annotate-empty", role: "status",
                "No subtasks yet. Run the annotation to fill this table." }
        };
    }
    rsx! {
        table { class: "build-annotate-table",
            caption { class: "sr-only", "Annotated subtasks by episode" }
            thead {
                tr {
                    th { scope: "col", "Start" }
                    th { scope: "col", "End" }
                    th { scope: "col", "Description" }
                }
            }
            for group in groups {
                tbody { key: "{group.episode}", class: "build-annotate-group",
                    "aria-current": (selected() == Some(group.episode.clone())).then_some("true"),
                    tr {
                        th { scope: "colgroup", colspan: "3",
                            button { r#type: "button", class: "build-annotate-episode-name",
                                title: "Show this episode below",
                                onclick: {
                                    let episode = group.episode.clone();
                                    move |_| selected.set(Some(episode.clone()))
                                },
                                "Episode {group.episode}"
                            }
                            span { class: "build-annotate-count",
                                "{group.rows().len()} subtask(s)" }
                        }
                    }
                    for row in group.rows() {
                        tr { key: "{row.start_time()}",
                            td { "{row.start_time().clock_label()}" }
                            td { "{row.end_time().clock_label()}" }
                            td {
                                button { r#type: "button", class: "build-annotate-row",
                                    title: "Play this subtask",
                                    onclick: {
                                        let episode = group.episode.clone();
                                        let span = (row.start_time(), row.end_time());
                                        move |_| {
                                            selected.set(Some(episode.clone()));
                                            play.set(Some(span));
                                        }
                                    },
                                    "{row.description()}"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The selected episode's cameras, with its subtasks on the shared timeline.
#[component]
pub(super) fn AnnotatedEpisode(
    episode: AnnotationEpisode,
    subtasks: EpisodeSubtasks,
    play: Signal<Option<(f64, f64)>>,
) -> Element {
    let transport = EpisodeTransport::use_opened_episode(episode.opened());
    let track = subtasks.track();
    let requested = transport.clone();
    use_effect(move || {
        if let Some((start, end)) = play() {
            let mut transport = requested.clone();
            transport.play_interval(start, end);
            play.set(None);
        }
    });
    let activated = transport.clone();
    rsx! {
        section { class: "build-annotate-episode", "aria-label": "Annotated episode",
            EpisodeReview {
                episode: episode.opened().clone(),
                transport,
                span_tracks: if track.is_empty() { Vec::new() } else { vec![track] },
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
}
