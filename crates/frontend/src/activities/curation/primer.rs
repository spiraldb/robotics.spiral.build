//! What a measurement is for, shown until it has measured something.
//!
//! A node that has read nothing has no index to list and no recording that
//! means anything yet, so its workspace says what the node does and why a
//! curation wants it: a drawing that plays the idea once, two sentences, and
//! the run that starts it. The drawing is the argument; the words only name
//! what it showed.
use dioxus::prelude::*;

/// The measurements that explain themselves before their first run.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Primer {
    BreakDown,
    ScoreProgress,
}

impl Primer {
    /// What the drawing shows, as the problem it answers.
    fn lead(self) -> &'static str {
        match self {
            Self::BreakDown => {
                "An episode is rarely one task: folding a towel is reach, grasp, fold, and place."
            }
            Self::ScoreProgress => {
                "Even good demonstrations hesitate, regrasp, and retry, and training on those \
                 moments can make a policy worse."
            }
        }
    }
    /// What the node does about it.
    fn answer(self) -> &'static str {
        match self {
            Self::BreakDown => {
                "Break down splits every episode into its steps, so you can pick the same step \
                 out of all of them."
            }
            Self::ScoreProgress => {
                "Score progress rates every moment against the expert rate and leaves out the \
                 stretches that stall. The rest of the episode still trains."
            }
        }
    }
}

/// A measurement's workspace before its first run: the drawing, the two
/// sentences, and `action` — the run, or what it is doing while the first
/// episodes are read.
#[component]
pub(super) fn NodePrimer(primer: Primer, action: Element) -> Element {
    rsx! {
        div { class: "curation-primer",
            figure { class: "curation-primer-figure", "aria-hidden": "true",
                match primer {
                    Primer::BreakDown => rsx! { BreakDownDrawing {} },
                    Primer::ScoreProgress => rsx! { ScoreProgressDrawing {} },
                }
            }
            div { class: "curation-primer-words",
                p { class: "curation-primer-lead", "{primer.lead()}" }
                p { "{primer.answer()}" }
            }
            div { class: "curation-primer-action", {action} }
        }
    }
}

/// One episode of the Break down drawing: its row, how far it slides so its
/// fold lines up with the first episode's, and its four steps as `(x, width)`.
struct DrawnEpisode {
    y: f64,
    align: f64,
    steps: [(f64, f64); 4],
}

impl DrawnEpisode {
    /// Three takes of one task, each timed differently, so their folds start
    /// at three different moments until the drawing lines them up.
    const ALL: [Self; 3] = [
        Self {
            y: 18.0,
            align: 0.0,
            steps: [(24.0, 72.0), (98.0, 90.0), (190.0, 162.0), (354.0, 90.0)],
        },
        Self {
            y: 74.0,
            align: 60.0,
            steps: [(24.0, 46.0), (72.0, 56.0), (130.0, 132.0), (264.0, 120.0)],
        },
        Self {
            y: 108.0,
            align: -46.0,
            steps: [(56.0, 80.0), (138.0, 96.0), (236.0, 128.0), (366.0, 78.0)],
        },
    ];
    /// The step the drawing picks out of every episode.
    const PICKED: usize = 2;
    /// The first episode's steps, named under it.
    const NAMES: [(f64, &str); 4] = [
        (60.0, "reach"),
        (143.0, "grasp"),
        (271.0, "fold"),
        (399.0, "place"),
    ];

    /// Where the whole take runs, for the bar it is recorded as before it
    /// splits.
    fn span(&self) -> (f64, f64) {
        let (first, _) = self.steps[0];
        let (last, width) = self.steps[3];
        (first, last + width - first)
    }
}

/// One episode recorded as one bar, split into its steps; two more takes
/// arrive, slide until their folds line up, and the fold of every take is
/// picked out together.
#[component]
fn BreakDownDrawing() -> Element {
    rsx! {
        svg { class: "curation-primer-breakdown", view_box: "0 0 480 172",
            for (row, episode) in DrawnEpisode::ALL.iter().enumerate() {
                g {
                    key: "{row}",
                    class: "curation-primer-take",
                    style: "--row: {row}; --align: {episode.align}px",
                    g { class: "curation-primer-episode",
                        rect {
                            class: "curation-primer-track",
                            x: "{episode.span().0}", y: "{episode.y}",
                            width: "{episode.span().1}", height: "22", rx: "4",
                        }
                        for (step, (x, width)) in episode.steps.iter().enumerate() {
                            rect {
                                key: "{step}",
                                class: "curation-primer-step",
                                "data-picked": step == DrawnEpisode::PICKED,
                                style: "--step: {step}",
                                x: "{x}", y: "{episode.y}", width: "{width}", height: "22", rx: "4",
                            }
                        }
                    }
                    if row == 0 {
                        g { class: "curation-primer-names",
                            for (step, (x, name)) in DrawnEpisode::NAMES.iter().enumerate() {
                                text {
                                    key: "{name}",
                                    "data-picked": step == DrawnEpisode::PICKED,
                                    style: "--step: {step}",
                                    x: "{x}", y: "56",
                                    "{name}"
                                }
                            }
                        }
                    }
                }
            }
            rect { class: "curation-primer-pick", x: "183", y: "10", width: "176", height: "128", rx: "7" }
            text { class: "curation-primer-pick-name", x: "271", y: "158", "the same step, from every episode" }
        }
    }
}

/// An episode's progress rate drawn as the timeline draws it — the 1× expert
/// rate, the dashed cutoff — dipping under the cutoff while the gripper
/// regrasps; that stretch washes out and leaves the episode, whose other
/// stretches are kept.
#[component]
fn ScoreProgressDrawing() -> Element {
    rsx! {
        svg { class: "curation-primer-progress", view_box: "0 0 480 176",
            rect { class: "curation-primer-left-out", x: "196", y: "8", width: "88", height: "150" }
            line { class: "curation-primer-reference", x1: "24", x2: "456", y1: "30", y2: "30" }
            text { class: "curation-primer-axis", x: "456", y: "24", "1× expert rate" }
            line { class: "curation-primer-cutoff", x1: "24", x2: "456", y1: "62", y2: "62" }
            text { class: "curation-primer-axis", "data-cutoff": "true", x: "456", y: "56", "cutoff" }
            path {
                class: "curation-primer-rate",
                "pathLength": "1",
                d: "M24 40 C 44 30, 64 26, 84 30 S 124 38, 144 32 S 180 26, 196 50 \
                    C 206 76, 214 92, 226 84 S 244 64, 252 88 S 272 96, 284 58 \
                    C 292 38, 306 30, 326 32 S 366 26, 386 32 S 426 36, 456 30",
            }
            text { class: "curation-primer-stall", x: "240", y: "118", "regrasp" }
            rect { class: "curation-primer-kept", x: "24", y: "132", width: "170", height: "22", rx: "4" }
            rect { class: "curation-primer-cut", x: "198", y: "132", width: "84", height: "22", rx: "4" }
            rect { class: "curation-primer-kept", x: "286", y: "132", width: "170", height: "22", rx: "4" }
            text { class: "curation-primer-side", x: "109", y: "172", "kept" }
            text { class: "curation-primer-side", "data-left-out": "true", x: "240", y: "172", "left out" }
            text { class: "curation-primer-side", x: "371", y: "172", "kept" }
        }
    }
}
