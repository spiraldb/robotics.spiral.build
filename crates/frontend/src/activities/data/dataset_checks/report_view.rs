//! How the report reads: its bottom line first — how many checks came out
//! each way, which also says what each mark means — then the checks in two
//! sections, the recording's streams and the arms' joints. Each check is one
//! row whose form says how it came out, the readings it found, where among
//! the dataset's episodes it holds, and on demand the detail behind it: the
//! streams, arms and episodes it names. An episode it names opens where the
//! finding is, through the surface's own episode tabs.
use crate::activities::episode::{Episode, SourceLabel};
use crate::components::icons::{CameraStreamIcon, SignalStreamIcon};
use crate::format::ClockPresentation as _;
use dioxus::prelude::*;
use spiral_robotics_common::datasets::EpisodeId;
use spiral_robotics_ui::{CheckIcon, Disclosure};
use std::collections::HashMap;

/// How a check came out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Verdict {
    /// Nothing out of the ordinary.
    Clean,
    /// Something an operator should look at.
    Flagged,
    /// A measurement with no fault to find, such as how grippers close.
    Measured,
    /// The dataset lacks what the check reads.
    Unavailable,
}

impl Verdict {
    fn key(self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::Flagged => "flagged",
            Self::Measured => "measured",
            Self::Unavailable => "unavailable",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Clean => "Clean",
            Self::Flagged => "Flagged",
            Self::Measured => "Measured",
            Self::Unavailable => "Not checked",
        }
    }
}

/// What a check found, as its row says it: each reading a value, set as the
/// measurement it is, and the few words after it that say what it measures,
/// read on as one line — or, when there was nothing to measure, why.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Finding {
    readings: Vec<Reading>,
    reason: Option<String>,
}

impl Finding {
    /// A finding that leads with `value`, `words` saying what it is.
    pub(super) fn of(value: impl Into<String>, words: &str) -> Self {
        Self::led_by(Reading::new(value.into(), words))
    }
    /// A finding that leads with how many of `noun` there are, the noun
    /// agreeing with the count and `words` after it.
    pub(super) fn of_count(count: usize, noun: &str, words: &str) -> Self {
        Self::led_by(Reading::counted(count, noun, words))
    }
    /// Why the check measured nothing.
    pub(super) fn why(reason: impl Into<String>) -> Self {
        Self {
            readings: Vec::new(),
            reason: Some(reason.into()),
        }
    }
    /// One more reading after the ones before it.
    pub(super) fn and(mut self, value: impl Into<String>, words: &str) -> Self {
        self.readings.push(Reading::new(value.into(), words));
        self
    }
    /// One more count after the readings before it.
    pub(super) fn and_count(mut self, count: usize, noun: &str, words: &str) -> Self {
        self.readings.push(Reading::counted(count, noun, words));
        self
    }
    fn led_by(reading: Reading) -> Self {
        Self {
            readings: vec![reading],
            reason: None,
        }
    }
    #[cfg(test)]
    pub(super) fn text(&self) -> String {
        self.reason.clone().unwrap_or_else(|| {
            self.readings
                .iter()
                .map(|reading| format!("{}{}", reading.value, reading.tail))
                .collect::<Vec<_>>()
                .join(" ")
        })
    }
}

/// One value a check read, and the words after it.
#[derive(Clone, Debug, PartialEq)]
struct Reading {
    value: String,
    tail: String,
}

impl Reading {
    fn new(value: String, words: &str) -> Self {
        Self {
            value,
            tail: Self::after(words),
        }
    }
    /// `count`, then `noun` agreeing with it, then `words`.
    fn counted(count: usize, noun: &str, words: &str) -> Self {
        Self {
            value: count.to_string(),
            tail: format!(" {}{}", Plural::noun(count, noun), Self::after(words)),
        }
    }
    /// `words` as they follow what comes before them: after a space, unless
    /// they open with punctuation that belongs against it.
    fn after(words: &str) -> String {
        match words {
            "" => String::new(),
            words if words.starts_with([',', ';', ':']) => words.to_owned(),
            words => format!(" {words}"),
        }
    }
}

/// A noun agreeing with its count.
pub(super) struct Plural;

impl Plural {
    /// `count` and its noun: "1 episode", "3 episodes".
    pub(super) fn of(count: usize, noun: &str) -> String {
        format!("{count} {}", Self::noun(count, noun))
    }
    fn noun(count: usize, noun: &str) -> String {
        if count == 1 {
            noun.to_owned()
        } else {
            format!("{noun}s")
        }
    }
}

/// One check as the report lists it.
#[derive(Clone, PartialEq)]
pub(super) struct CheckRow {
    name: &'static str,
    verdict: Verdict,
    finding: Finding,
    details: Option<Element>,
    /// The episodes the finding holds for, each where it holds.
    flagged: Vec<Evidence>,
}

impl CheckRow {
    pub(super) fn new(name: &'static str, verdict: Verdict, finding: Finding) -> Self {
        Self {
            name,
            verdict,
            finding,
            details: None,
            flagged: Vec::new(),
        }
    }
    pub(super) fn with_details(mut self, details: Element) -> Self {
        self.details = Some(details);
        self
    }
    /// The episodes a flagged finding holds for, which the row lights on
    /// the dataset's strip of episodes. An episode named twice keeps its
    /// first evidence.
    pub(super) fn flagging(mut self, evidence: impl IntoIterator<Item = Evidence>) -> Self {
        self.flagged = evidence.into_iter().collect();
        self
    }
}

/// What a section's checks read, when that needs saying, and on demand the
/// detail behind it.
#[derive(Clone, PartialEq)]
pub(super) struct SectionAbout {
    summary: String,
    details: Option<Element>,
}

impl SectionAbout {
    pub(super) fn new(summary: String) -> Self {
        Self {
            summary,
            details: None,
        }
    }
    pub(super) fn with_details(mut self, details: Element) -> Self {
        self.details = Some(details);
        self
    }
}

/// A run of checks that read one kind of stream.
#[derive(Clone, PartialEq)]
pub(super) struct CheckSection {
    name: &'static str,
    about: Option<SectionAbout>,
    rows: Vec<CheckRow>,
}

impl CheckSection {
    pub(super) fn new(name: &'static str, rows: Vec<CheckRow>) -> Self {
        Self {
            name,
            about: None,
            rows,
        }
    }
    pub(super) fn about(mut self, about: SectionAbout) -> Self {
        self.about = Some(about);
        self
    }
}

/// One episode a finding names, what it measured there, and the moment in
/// it worth opening at.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Evidence {
    episode: EpisodeId,
    moment: Option<f64>,
    measure: String,
}

impl Evidence {
    /// The episode as a whole.
    pub(super) fn whole(episode: EpisodeId, measure: String) -> Self {
        Self {
            episode,
            moment: None,
            measure,
        }
    }
    /// The episode at `moment` seconds in, when it has one.
    pub(super) fn at(episode: EpisodeId, moment: Option<f64>, measure: String) -> Self {
        Self {
            episode,
            moment: moment.filter(|moment| moment.is_finite() && *moment >= 0.0),
            measure,
        }
    }
    #[cfg(test)]
    pub(super) fn measure(&self) -> &str {
        &self.measure
    }
    /// The episode and what was measured in it, as its chip reads.
    fn readout(&self) -> String {
        let name = Episode::listed_name_of(&self.episode);
        if self.measure.is_empty() {
            name
        } else {
            format!("{name} {}", self.measure)
        }
    }
    fn title(&self) -> String {
        match self.moment {
            Some(moment) => format!(
                "Open {} at {}",
                Episode::name_of(&self.episode),
                moment.clock_label()
            ),
            None => format!("Open {}", Episode::name_of(&self.episode)),
        }
    }
}

/// An episode the report opens: the episode, the label the dataset's list
/// gives it, and where to play from.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::activities) struct CheckedEpisode {
    episode: Episode,
    label: SourceLabel,
    moment: Option<f64>,
}

impl CheckedEpisode {
    pub(in crate::activities) fn episode(&self) -> &Episode {
        &self.episode
    }
    pub(in crate::activities) fn label(&self) -> &SourceLabel {
        &self.label
    }
    pub(in crate::activities) fn moment(&self) -> Option<f64> {
        self.moment
    }
}

/// What opens an episode a finding names, for every chip of the report.
#[derive(Clone, Copy)]
pub(super) struct EvidenceOpener {
    episodes: ReadSignal<Option<Vec<(Episode, SourceLabel)>>>,
    on_open: EventHandler<CheckedEpisode>,
}

impl EvidenceOpener {
    pub(super) fn provide(
        episodes: ReadSignal<Option<Vec<(Episode, SourceLabel)>>>,
        on_open: EventHandler<CheckedEpisode>,
    ) {
        use_context_provider(|| Self { episodes, on_open });
    }
    fn open(self, evidence: &Evidence) {
        let listed = self.episodes.read();
        let Some((episode, label)) = listed
            .iter()
            .flatten()
            .find(|(episode, _)| episode.id() == &evidence.episode)
        else {
            return;
        };
        self.on_open.call(CheckedEpisode {
            episode: episode.clone(),
            label: label.clone(),
            moment: evidence.moment,
        });
    }
}

/// The report: its bottom line, then each section's checks in the order
/// every dataset lists them.
#[component]
pub(super) fn CheckReport(sections: Vec<CheckSection>) -> Element {
    let verdicts = sections
        .iter()
        .flat_map(|section| section.rows.iter().map(|row| row.verdict))
        .collect::<Vec<_>>();
    // Rows arrive one after another down the whole report, not per section.
    let mut order = 0usize;
    let sections = sections
        .into_iter()
        .map(|section| {
            let first = order;
            order += section.rows.len() + 1;
            (first, section)
        })
        .collect::<Vec<_>>();
    rsx! {
        div { class: "data-checks-report",
            CheckTally { verdicts }
            for (first, section) in sections {
                section { key: "{section.name}", class: "data-checks-section", "aria-label": "{section.name}",
                    SectionHead { name: section.name, about: section.about.clone(), order: first }
                    if !section.rows.is_empty() {
                        ol { class: "data-checks-list",
                            for (index, row) in section.rows.into_iter().enumerate() {
                                CheckItem { key: "{row.name}", row, order: first + 1 + index }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// How many checks came out each way: the report's bottom line, drawn with
/// the marks the rows wear, so it also says what each mark means. Only what
/// there is to look at is set in ink; the rest rests.
#[component]
fn CheckTally(verdicts: Vec<Verdict>) -> Element {
    let count = |verdict: Verdict| verdicts.iter().filter(|each| **each == verdict).count();
    let parts = [
        (Verdict::Flagged, "to look at"),
        (Verdict::Clean, "clean"),
        (Verdict::Measured, "measured"),
        (Verdict::Unavailable, "not checked"),
    ]
    .into_iter()
    .map(|(verdict, words)| (verdict, count(verdict), words))
    .filter(|(_, count, _)| *count > 0)
    .collect::<Vec<_>>();
    rsx! {
        p { class: "data-checks-tally",
            for (verdict, count, words) in parts {
                span { key: "{verdict.key()}", class: "data-checks-tally-part", "data-verdict": verdict.key(),
                    VerdictMark { verdict }
                    span { class: "data-checks-tally-count", "{count}" }
                    span { "{words}" }
                }
            }
        }
    }
}

/// A section's name, and what its checks read when that needs saying.
#[component]
fn SectionHead(name: &'static str, about: Option<SectionAbout>, order: usize) -> Element {
    rsx! {
        header { class: "data-checks-section-head", style: "--i: {order}",
            h3 { "{name}" }
            match about {
                Some(SectionAbout { summary, details: Some(details) }) => rsx! {
                    Disclosure { class: "data-checks-about",
                        summary: rsx! { span { class: "data-checks-about-text", "{summary}" } },
                        div { class: "data-checks-details", {details} }
                    }
                },
                Some(SectionAbout { summary, details: None }) => rsx! {
                    p { class: "data-checks-about-text", "{summary}" }
                },
                None => rsx! {},
            }
        }
    }
}

#[component]
fn CheckItem(row: CheckRow, order: usize) -> Element {
    let summary = rsx! {
        CheckSummary { name: row.name, verdict: row.verdict, finding: row.finding.clone(),
            flagged: if row.verdict == Verdict::Flagged { row.flagged.clone() } else { Vec::new() } }
    };
    rsx! {
        li { class: "data-checks-item", "data-verdict": row.verdict.key(), style: "--i: {order}",
            match row.details {
                Some(details) => rsx! {
                    Disclosure { class: "data-checks-row", summary,
                        div { class: "data-checks-details", {details} }
                    }
                },
                None => rsx! { div { class: "data-checks-row is-plain", {summary} } },
            }
        }
    }
}

/// The mark a check wears for how it came out: a check when clean, a
/// caution diamond when flagged, a ring for a measurement, a dashed ring
/// when the dataset lacks what the check reads.
#[component]
pub(super) fn VerdictMark(verdict: Verdict) -> Element {
    rsx! {
        span { class: "data-checks-mark", "data-verdict": verdict.key(),
            title: "{verdict.label()}", "aria-label": "{verdict.label()}",
            if verdict == Verdict::Clean { CheckIcon {} }
        }
    }
}

#[component]
fn CheckSummary(
    name: &'static str,
    verdict: Verdict,
    finding: Finding,
    flagged: Vec<Evidence>,
) -> Element {
    rsx! {
        VerdictMark { verdict }
        span { class: "data-checks-name", "{name}" }
        span { class: "data-checks-summary",
            match finding.reason {
                Some(reason) => rsx! { "{reason}" },
                None => rsx! {
                    // A value never breaks from its unit.
                    for (index, reading) in finding.readings.into_iter().enumerate() {
                        if index > 0 { " " }
                        span { class: "data-checks-value", {reading.value.replace(' ', "\u{a0}")} }
                        "{reading.tail}"
                    }
                },
            }
        }
        if !flagged.is_empty() {
            EpisodeStrip { flagged }
        }
    }
}

/// Where a finding holds among the dataset's episodes, in the order the
/// dataset lists them, oldest first: one cell per episode, lit where the
/// finding holds. A glance tells a fault in two takes from one in all of
/// them, and when in the recording's life it began. A lit cell opens its
/// episode where the finding is; the detail's chips are the same episodes
/// for the keyboard.
#[component]
fn EpisodeStrip(flagged: Vec<Evidence>) -> Element {
    let opener = use_context::<EvidenceOpener>();
    // The lit cell under the pointer reads out in place of the count.
    let mut pointed = use_signal(|| None::<String>);
    let mut lit = HashMap::new();
    for evidence in &flagged {
        lit.entry(evidence.episode.clone())
            .or_insert_with(|| evidence.clone());
    }
    // Each episode in the dataset's order, and for a lit one the order it
    // lights in, what it says under the pointer, and where it opens.
    let mut lit_so_far = 0usize;
    let cells = opener
        .episodes
        .read()
        .iter()
        .flatten()
        .map(|(episode, _)| {
            let evidence = lit.get(episode.id()).cloned();
            let order = evidence.as_ref().map(|_| {
                lit_so_far += 1;
                lit_so_far
            });
            let title = evidence.as_ref().map(Evidence::title);
            let readout = evidence.as_ref().map(Evidence::readout);
            (episode.id().clone(), order, title, readout, evidence)
        })
        .collect::<Vec<_>>();
    let total = cells.len();
    if total == 0 || lit_so_far == 0 {
        return rsx! {};
    }
    // Room for a hairline between cells while each stays three pixels wide.
    let gap = if total <= 60 { 1 } else { 0 };
    rsx! {
        span { class: "data-checks-scope",
            span { class: "data-checks-strip", "aria-hidden": "true",
                style: "--episodes: {total}; --strip-gap: {gap}px",
                onmouseleave: move |_| pointed.set(None),
                for (episode, order, title, readout, evidence) in cells {
                    i { key: "{episode}",
                        class: if order.is_some() { "is-lit" },
                        style: order.map(|order| format!("--c: {order}")),
                        title,
                        onmouseenter: move |_| pointed.set(readout.clone()),
                        onclick: move |event: MouseEvent| {
                            if let Some(evidence) = &evidence {
                                // A cell sits in the row's summary; opening an
                                // episode must not also fold the row.
                                event.prevent_default();
                                event.stop_propagation();
                                opener.open(evidence);
                            }
                        }
                    }
                }
            }
            match pointed() {
                Some(readout) => rsx! { span { class: "data-checks-scope-count is-pointed", "{readout}" } },
                None => rsx! { span { class: "data-checks-scope-count", "{lit_so_far} of {total}" } },
            }
        }
    }
}

/// The episodes a finding names, each opening where the finding is.
#[component]
pub(super) fn EvidenceChips(evidence: Vec<Evidence>) -> Element {
    if evidence.is_empty() {
        return rsx! { span { class: "data-checks-none", "—" } };
    }
    let opener = use_context::<EvidenceOpener>();
    let ready = opener.episodes.read().is_some();
    rsx! {
        span { class: "data-checks-chips",
            for item in evidence {
                button { key: "{item.episode}-{item.measure}",
                    r#type: "button", class: "data-checks-chip", disabled: !ready,
                    title: item.title(),
                    onclick: {
                        let item = item.clone();
                        move |_| opener.open(&item)
                    },
                    span { class: "data-checks-chip-id", "{Episode::listed_name_of(&item.episode)}" }
                    if !item.measure.is_empty() {
                        span { class: "data-checks-chip-measure", "{item.measure}" }
                    }
                }
            }
        }
    }
}

/// A stream by its topic, marked as a camera or a data stream.
#[component]
pub(super) fn StreamName(topic: String, camera: bool) -> Element {
    rsx! {
        span { class: "data-ledger-stream",
            if camera {
                span { class: "data-ledger-glyph is-camera", title: "Camera", CameraStreamIcon {} }
            } else {
                span { class: "data-ledger-glyph", title: "Data", SignalStreamIcon {} }
            }
            span { "{topic}" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_finding_leads_with_its_values_and_its_nouns_agree() {
        let gaps = Finding::of_count(0, "frame", "missing,").and("33 ms", "longest gap");
        assert_eq!(gaps.text(), "0 frames missing, 33 ms longest gap");
        let closes = Finding::of_count(1, "close", ":").and("1", "all the way");
        assert_eq!(closes.text(), "1 close: 1 all the way");
        assert_eq!(
            Finding::of_count(2, "stream", "at another rate in")
                .and_count(3, "episode", "")
                .text(),
            "2 streams at another rate in 3 episodes"
        );
        assert_eq!(
            Finding::why("No camera to pair").text(),
            "No camera to pair"
        );
        assert_eq!(Plural::of(1, "episode"), "1 episode");
    }
}
