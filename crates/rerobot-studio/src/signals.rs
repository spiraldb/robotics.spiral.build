//! Joint angles over time: the one place the studio plots a measurement.
//!
//! Everything else in the studio answers "where is this arm now". A take is the
//! one thing that has a *shape* — a reach, a grasp, a lift — and a column of
//! numbers refreshing twenty times a second cannot show a shape. So the signals
//! panel exists for exactly the span of a take: it is empty until Record, fills
//! while the take runs, and holds the finished take afterwards so the operator
//! can look at what they just captured before deciding to record another.
//!
//! Two properties keep it honest as a take gets long. The samples live here in
//! Rust, capped and trimmed from the front, so a long session cannot grow the
//! page without bound. And nothing is handed to the renderer that the renderer
//! could not draw: the visible window is reduced to a point budget by extremes,
//! never by averaging, so a spike survives being zoomed out the same way it
//! survives in the recorded file.

use dioxus::prelude::*;
use rerobot_core::api::TelemetryFrame;
use rerobot_core::arm;

use crate::icons::SignalsPanelIcon;
use crate::state::studio;
use crate::viewport::accent_css;

/// The element the plots are painted into.
pub const CANVAS_ID: &str = "signals-plot";

/// Samples kept for one take before the oldest are dropped.
///
/// Thirty minutes at the studio's 20 Hz publish rate. A take that long is
/// already unusual; one long enough to exhaust a browser tab is not something
/// to discover during a session, so the ceiling is fixed and the loss is
/// reported rather than silent.
const MAX_SAMPLES: usize = 36_000;

/// How much is dropped when the cap is reached, so trimming is amortized
/// instead of shifting every vector on every frame.
const TRIM_CHUNK: usize = 3_600;

/// Points per series handed to the renderer for the visible window.
///
/// Roughly two per horizontal pixel at the widest this panel gets. Past this
/// the canvas cannot resolve the difference, and the only thing another
/// thousand points buys is a slower frame. A bucket that also held a dropout
/// contributes a third point, so the real ceiling is 1.5× this.
const POINT_BUDGET: usize = 720;

/// The categorical slots joints are drawn in, in kinematic order from the base
/// of the arm to the gripper.
///
/// Six fixed hues assigned in a fixed order — the order is what keeps adjacent
/// pairs apart under protanopia and deuteranopia, so it is not rearranged for
/// looks. Verified as a set against this surface: worst adjacent pair ΔE 9.1
/// simulated, 19.6 unsimulated. Three of them sit below 3:1 against paper,
/// which is why the readout under the plots prints every value as text rather
/// than leaving a 1.5px line to carry it alone.
///
/// These live here rather than in `tailwind.css` for the same reason the arm
/// accents live in `viewport.rs`: they are painted into a canvas, and the only
/// HTML that spends one is the key beside the value it is labelling.
const SIGNAL_COLORS: [&str; 6] = [
    "#2a78d6", // shoulder pan
    "#eb6834", // shoulder lift
    "#1baf7a", // elbow flex
    "#eda100", // wrist flex
    "#e87ba4", // wrist roll
    "#008300", // gripper
];

/// The colour a joint is drawn in, taken from its place in the arm.
///
/// Keyed on the joint's own name rather than on its position in whatever list
/// this arm happens to report, so an arm missing a motor does not repaint every
/// joint below it — the same rule the arm accents already follow.
pub fn signal_color(name: &str) -> &'static str {
    let slot = arm::SO101_JOINTS
        .iter()
        .position(|joint| joint.name == name)
        .unwrap_or_else(|| {
            name.bytes().fold(0usize, |hash, byte| {
                hash.wrapping_mul(31).wrapping_add(usize::from(byte))
            }) % SIGNAL_COLORS.len()
        });
    SIGNAL_COLORS[slot % SIGNAL_COLORS.len()]
}

/// How a joint is named on screen.
pub fn joint_label(name: &str) -> String {
    arm::so101_joint(name).map_or_else(|| name.to_owned(), |joint| joint.label.to_owned())
}

/// One joint's measured angle for every sample in the take.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JointSignal {
    pub name: String,
    /// Degrees from the calibrated centre, `None` where the joint had no
    /// calibrated reading for that sample.
    pub values: Vec<Option<f32>>,
}

/// One arm's joints, in the order the arm reports them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ArmSignals {
    pub id: String,
    pub joints: Vec<JointSignal>,
}

/// Every sample of the take being recorded, or of the last one recorded.
///
/// One shared time column: every arm in a take is sampled by the same control
/// loop on the same frame, so two arms cannot disagree about when a sample
/// happened — and storing the column once is what lets a cursor land on one
/// index and read every arm at that instant.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SignalStore {
    /// The take these samples belong to, so a new recording replaces them
    /// rather than continuing into them.
    take: Option<String>,
    /// Seconds since the take started.
    times: Vec<f32>,
    arms: Vec<ArmSignals>,
    /// Samples dropped off the front to stay inside the cap.
    trimmed: usize,
    /// True while the take is still being captured.
    live: bool,
    /// Consecutive frames that reported no take.
    ///
    /// One is a publishing race between the recorder's state and the frame that
    /// carries it; two is the take having actually ended. The distinction
    /// matters because "the take ended" is one of the conditions that wipes
    /// these samples, and a take wiped by a single dropped frame restarts from
    /// zero in front of an operator who is still performing it.
    idle: u8,
}

impl SignalStore {
    pub fn arms(&self) -> &[ArmSignals] {
        &self.arms
    }

    pub fn times(&self) -> &[f32] {
        &self.times
    }

    pub fn len(&self) -> usize {
        self.times.len()
    }

    pub fn is_empty(&self) -> bool {
        self.times.is_empty()
    }

    pub fn take(&self) -> Option<&str> {
        self.take.as_deref()
    }

    pub fn live(&self) -> bool {
        self.live
    }

    pub fn trimmed(&self) -> usize {
        self.trimmed
    }

    /// The span the take covers, oldest kept sample to newest.
    pub fn extent(&self) -> Option<(f32, f32)> {
        Some((*self.times.first()?, *self.times.last()?))
    }

    /// Fold one telemetry frame in, starting or ending a take as the server
    /// reports one.
    ///
    /// Driven from the event stream rather than from the panel, because a take
    /// recorded while the panel is collapsed is still a take: whether a chart
    /// is on screen does not get to decide what was measured.
    pub fn observe(&mut self, frame: &TelemetryFrame) {
        let Some(recording) = frame.recording.as_ref() else {
            self.idle = self.idle.saturating_add(1);
            if self.idle >= 2 {
                self.live = false;
            }
            return;
        };
        self.idle = 0;
        // Three ways a frame can belong to a take these samples are not from,
        // and only the first is the obvious one. A restarted server numbers its
        // episodes from one again, so the id alone would let a fresh take append
        // into the previous one — and an appended take is not merely wrong, it
        // is *unsorted*, which quietly turns both binary searches over the time
        // column into nonsense and draws the trace backwards.
        let renumbered = self.take.as_deref() != Some(recording.episode.as_str());
        let resumed = !self.live;
        let rewound = self
            .times
            .last()
            .is_some_and(|last| (recording.elapsed_seconds as f32) < *last);
        if renumbered || resumed || rewound {
            self.reset(&recording.episode);
        }
        self.live = true;

        // Elapsed comes from the recorder rather than from the browser clock:
        // it is the same number the take's own file is stamped with, so this
        // axis and the duration on the episode card cannot disagree.
        self.times.push(recording.elapsed_seconds as f32);
        let sample = self.times.len();

        for robot in &frame.robots {
            let arm = match self.arms.iter_mut().position(|arm| arm.id == robot.id) {
                Some(index) => &mut self.arms[index],
                None => {
                    self.arms.push(ArmSignals {
                        id: robot.id.clone(),
                        joints: Vec::new(),
                    });
                    self.arms.last_mut().expect("just pushed")
                }
            };
            for joint in &robot.joints {
                let signal = match arm.joints.iter().position(|held| held.name == joint.name) {
                    Some(index) => &mut arm.joints[index],
                    None => {
                        arm.joints.push(JointSignal {
                            name: joint.name.clone(),
                            // An arm that appears mid-take starts with a gap
                            // rather than with its first reading stretched
                            // backwards over time it was not present for.
                            values: vec![None; sample - 1],
                        });
                        arm.joints.last_mut().expect("just pushed")
                    }
                };
                signal.values.push(joint.degrees.map(|degrees| degrees as f32));
            }
        }

        // Any joint that did not report this frame gets a hole, so every series
        // stays index-aligned with the shared time column.
        for arm in &mut self.arms {
            for joint in &mut arm.joints {
                joint.values.resize(sample, None);
            }
        }

        if self.times.len() > MAX_SAMPLES {
            self.trim();
        }
    }

    fn reset(&mut self, take: &str) {
        self.take = Some(take.to_owned());
        self.times.clear();
        self.arms.clear();
        self.trimmed = 0;
        self.idle = 0;
    }

    fn trim(&mut self) {
        self.times.drain(..TRIM_CHUNK);
        for arm in &mut self.arms {
            for joint in &mut arm.joints {
                joint.values.drain(..TRIM_CHUNK);
            }
        }
        self.trimmed += TRIM_CHUNK;
    }

    /// Index range covering `[start, end]`, with one sample of overhang on each
    /// side so a line entering the window is drawn from off-screen rather than
    /// beginning abruptly at the edge.
    pub fn range(&self, start: f32, end: f32) -> (usize, usize) {
        let lower = self
            .times
            .partition_point(|time| *time < start)
            .saturating_sub(1);
        let upper = self.times.partition_point(|time| *time <= end);
        (lower, upper.clamp(lower, self.times.len()))
    }

    /// The sample nearest `time`, for reading every arm at one instant.
    pub fn nearest(&self, time: f32) -> Option<usize> {
        if self.times.is_empty() {
            return None;
        }
        let after = self.times.partition_point(|held| *held < time);
        if after == 0 {
            return Some(0);
        }
        if after >= self.times.len() {
            return Some(self.times.len() - 1);
        }
        let before = after - 1;
        Some(
            if (time - self.times[before]).abs() <= (self.times[after] - time).abs() {
                before
            } else {
                after
            },
        )
    }

    /// Every joint name in the take, base to gripper, without repeats.
    pub fn joint_names(&self) -> Vec<String> {
        let mut names = Vec::new();
        for arm in &self.arms {
            for joint in &arm.joints {
                if !names.contains(&joint.name) {
                    names.push(joint.name.clone());
                }
            }
        }
        names
    }

    /// One arm's angle for one joint at one sample.
    pub fn reading(&self, arm: &str, joint: &str, at: usize) -> Option<f32> {
        self.arms
            .iter()
            .find(|held| held.id == arm)?
            .joints
            .iter()
            .find(|held| held.name == joint)?
            .values
            .get(at)
            .copied()
            .flatten()
    }
}

/// How much of a take the plots are showing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Span {
    /// Trailing window that keeps up with the newest sample.
    #[default]
    Recent,
    Half,
    Long,
    /// Everything captured so far.
    All,
}

impl Span {
    pub const ALL: [Self; 4] = [Self::Recent, Self::Half, Self::Long, Self::All];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Recent => "10s",
            Self::Half => "30s",
            Self::Long => "2m",
            Self::All => "All",
        }
    }

    const fn seconds(self) -> Option<f32> {
        match self {
            Self::Recent => Some(10.0),
            Self::Half => Some(30.0),
            Self::Long => Some(120.0),
            Self::All => None,
        }
    }
}

/// The window the plots draw, and whether it is still tracking the newest
/// sample.
///
/// Following is the resting state and dragging the zoom leaves it, because the
/// two things an operator does with a running take — watch it arrive, and stop
/// to look at something — are exactly these two states. A scope that snapped
/// back to the live edge while being read would be unusable; one that never
/// came back would have to be re-zoomed after every take.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ViewWindow {
    Following(Span),
    Held { start: f32, end: f32 },
}

impl Default for ViewWindow {
    fn default() -> Self {
        Self::Following(Span::default())
    }
}

impl ViewWindow {
    pub fn span(self) -> Option<Span> {
        match self {
            Self::Following(span) => Some(span),
            Self::Held { .. } => None,
        }
    }

    /// Resolve to the seconds actually drawn, against what has been captured.
    pub fn resolve(self, extent: (f32, f32)) -> (f32, f32) {
        let (first, last) = extent;
        match self {
            Self::Following(span) => match span.seconds() {
                Some(seconds) => ((last - seconds).max(first), last.max(first + 0.001)),
                None => (first, last.max(first + 0.001)),
            },
            Self::Held { start, end } => (start, end.max(start + 0.001)),
        }
    }
}

/// Reduce `values` over `[lo, hi)` to at most `budget` points.
///
/// Reduced by extremes rather than by average, for the same reason the episode
/// store reduces its follow-error trace that way: the question a trace answers
/// is "how far did this go", and a mean is exactly the operation that hides the
/// answer. Each bucket contributes its lowest and highest sample, emitted in
/// the order they actually occurred so the line still runs forwards in time.
pub fn decimate(
    times: &[f32],
    values: &[Option<f32>],
    lo: usize,
    hi: usize,
    budget: usize,
) -> Vec<(f32, Option<f32>)> {
    let hi = hi.min(times.len()).min(values.len());
    if lo >= hi {
        return Vec::new();
    }
    let count = hi - lo;
    if count <= budget {
        return (lo..hi).map(|index| (times[index], values[index])).collect();
    }

    let buckets = (budget / 2).max(1);
    let mut points = Vec::with_capacity(buckets * 2);
    for bucket in 0..buckets {
        let start = lo + count * bucket / buckets;
        let end = (lo + count * (bucket + 1) / buckets).min(hi);
        if start >= end {
            continue;
        }
        let mut lowest: Option<(usize, f32)> = None;
        let mut highest: Option<(usize, f32)> = None;
        let mut gap: Option<usize> = None;
        for index in start..end {
            let Some(value) = values[index] else {
                gap = gap.or(Some(index));
                continue;
            };
            if lowest.is_none_or(|(_, held)| value < held) {
                lowest = Some((index, value));
            }
            if highest.is_none_or(|(_, held)| value > held) {
                highest = Some((index, value));
            }
        }
        match (lowest, highest) {
            (Some((low, _)), Some((high, _))) => {
                // At most three samples leave a bucket: its extremes, and — if
                // the bucket also held a hole — one hole, so a dropout survives
                // being zoomed out instead of being bridged by a line nobody
                // measured. All three are emitted in the order they happened,
                // because a series that steps backwards in time is drawn as a
                // stroke back across the plot.
                let mut chosen = [Some((low, values[low])), None, None];
                if high != low {
                    chosen[1] = Some((high, values[high]));
                }
                if let Some(hole) = gap {
                    chosen[2] = Some((hole, None));
                }
                let mut bucket = chosen.into_iter().flatten().collect::<Vec<_>>();
                bucket.sort_unstable_by_key(|(index, _)| *index);
                points.extend(
                    bucket
                        .into_iter()
                        .map(|(index, value)| (times[index], value)),
                );
            }
            _ => points.push((times[start], None)),
        }
    }
    points
}

/// Round a measured range out to whole degrees an operator reads without
/// counting, so every arm's plot lands on the same gridlines.
///
/// The step is chosen against the range *after* it is rounded outwards, not
/// before: picking it from the measurement and then widening the axis to the
/// nearest step is how a plot asked for four gridlines ends up drawing six.
/// `divisions` is what the plot has room for, so a 60px trace gets three
/// labels and a tall one gets five.
pub fn nice_range(low: f32, high: f32, divisions: u32) -> (f32, f32, f32) {
    const STEPS: [f32; 10] = [
        1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 45.0, 60.0, 90.0, 180.0,
    ];
    let divisions = divisions.max(1) as f32;
    let fits = |step: &f32| {
        let lower = (low / step).floor();
        let upper = (high / step).ceil();
        upper - lower <= divisions
    };
    let step = STEPS.into_iter().find(fits).unwrap_or(360.0);
    let lower = (low / step).floor() * step;
    let upper = (high / step).ceil() * step;
    if (upper - lower).abs() < f32::EPSILON {
        return (lower - step, upper + step, step);
    }
    (lower, upper, step)
}

/// Round seconds to label the time axis with, inside `[start, end]`.
///
/// Chosen here rather than left to the renderer, which labels the window's own
/// edge whenever there is room beside the first round tick — so a window that
/// happens to start at 180.31 seconds gets `180.31s` printed under it, in an
/// axis whose whole job is to be read at a glance.
pub fn time_ticks(start: f32, end: f32, target: u32) -> Vec<f32> {
    const STEPS: [f32; 13] = [
        0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0,
    ];
    let span = (end - start).max(1e-3);
    let target = target.max(1) as f32;
    let step = STEPS
        .into_iter()
        .find(|step| span / step <= target)
        .unwrap_or(1_800.0);
    let first = (start / step).ceil() as i64;
    let last = (end / step).floor() as i64;
    if last < first {
        return Vec::new();
    }
    (first..=last).map(|tick| tick as f32 * step).collect()
}

/// The lowest and highest angle any visible joint reaches in the window.
///
/// Shared by every arm's plot: a leader and a follower drawn on different
/// scales would put the same angle at two different heights, which is the one
/// comparison this panel exists to make easy.
pub fn shared_bounds(
    store: &SignalStore,
    lo: usize,
    hi: usize,
    hidden: &[String],
) -> Option<(f32, f32)> {
    let mut low = f32::INFINITY;
    let mut high = f32::NEG_INFINITY;
    for arm in store.arms() {
        for joint in &arm.joints {
            if hidden.iter().any(|name| name == &joint.name) {
                continue;
            }
            for value in joint.values.iter().take(hi).skip(lo).flatten() {
                low = low.min(*value);
                high = high.max(*value);
            }
        }
    }
    (low <= high).then_some((low, high))
}

#[cfg(feature = "web")]
mod plot;

/// The panel: a window control, the plots, and every joint read at one instant.
#[component]
pub fn SignalsPanel() -> Element {
    let studio = studio();
    let mut window = use_signal(ViewWindow::default);
    // What a preset returns to after the window has been dragged somewhere.
    let mut resume = use_signal(Span::default);
    let hidden = use_signal(Vec::<String>::new);
    // Where the time cursor is, when there is one. The plots publish it from
    // the pointer, the arrow keys move it, and the readout below reads every
    // arm at that instant.
    let mut cursor = use_signal(|| None::<f32>);
    // Which joint the operator is pointing at in the readout, so the trace it
    // labels answers. The readout is this panel's legend; a legend that does
    // not point at the thing it names is a table with colours in it.
    let pointed = use_signal(|| None::<String>);
    // False until the plotting runtime has actually been reached.
    let ready = use_signal(|| true);

    #[cfg(feature = "web")]
    plot::use_plot(studio, window, hidden, pointed, cursor, ready);

    let store = studio.signals.read();
    let empty = store.is_empty();
    let arms = store.arms().len();
    let live = store.live();
    let trimmed = store.trimmed() > 0;

    let heading = rsx! {
        SignalsHeading {
            window: window(),
            trimmed,
            on_span: move |span| {
                resume.set(span);
                window.set(ViewWindow::Following(span));
            },
            on_resume: move |()| window.set(ViewWindow::Following(resume())),
        }
    };

    if empty {
        return rsx! {
            section { class: "signals-panel",
                {heading}
                p { class: "signals-empty",
                    "Press "
                    strong { "Record" }
                    ", or "
                    strong { "R" }
                    ", to plot joint angles as they are captured."
                }
            }
        };
    }

    let extent = store.extent().unwrap_or((0.0, 1.0));
    let (start, end) = window().resolve(extent);
    let (lo, hi) = store.range(start, end);
    // With no pointer on the plots the readout holds the newest sample inside
    // the window: while a take runs that is the live value, and once it stops
    // it is where the arm finished. Either way the numbers are on the page
    // without anyone having to hover for them.
    let at = cursor()
        .and_then(|time| store.nearest(time))
        .unwrap_or_else(|| hi.saturating_sub(1).max(lo));
    let at_time = store.times().get(at).copied();
    // Arrow keys step a hundredth of the window, and a tenth of it with Shift —
    // both measured in samples, so both stay visible whatever the window is. A
    // single sample would be the precise choice and the wrong one: at 20 Hz it
    // is 0.05s, which the readout rounds away, so the key would appear dead.
    let span = hi.saturating_sub(lo);
    let (fine, coarse) = ((span / 100).max(1), (span / 10).max(2));
    let last_index = hi.saturating_sub(1).max(lo);
    let step = move |from: usize, forwards: bool, far: bool| -> Option<f32> {
        let amount = if far { coarse } else { fine };
        let next = if forwards {
            (from + amount).min(last_index)
        } else {
            from.saturating_sub(amount).max(lo)
        };
        studio.signals.peek().times().get(next).copied()
    };

    // Once a take has stopped, the plots are history and have to say whose.
    let held_take = (!live)
        .then(|| store.take().map(crate::timeline::short_id))
        .flatten();
    let names = store.joint_names();
    let columns: Vec<(String, &'static str)> = store
        .arms()
        .iter()
        .map(|arm| (arm.id.clone(), accent_css(studio.position_of(&arm.id))))
        .collect();
    let rows: Vec<ReadoutRow> = names
        .iter()
        .map(|name| ReadoutRow {
            name: name.clone(),
            label: joint_label(name),
            color: signal_color(name),
            values: columns
                .iter()
                .map(|(arm, _)| store.reading(arm, name, at))
                .collect(),
        })
        .collect();
    drop(store);

    rsx! {
        section { class: "signals-panel",
            {heading}
            div { class: "signals-stage",
                if !ready() {
                    // One hard dependency, and it is the only thing here that
                    // can be missing. Saying so is cheap; leaving an empty
                    // rectangle where two plots belong is not.
                    p { class: "signals-empty",
                        "The plotting runtime did not load, so the traces cannot be drawn. "
                        "Every joint's angle is still listed below."
                    }
                }
                // The renderer paints every plot into this one element; every
                // value it draws is also printed in the readout below it.
                // Focusable, because the crosshair is a pointer device and the
                // readout is the only way the plots' values are reachable.
                div {
                    id: CANVAS_ID,
                    class: "signals-canvas",
                    role: "img",
                    tabindex: 0,
                    "aria-label": "Joint angles over time for {arms} arm(s). Every value is listed in the readout below; arrow keys move the time cursor.",
                    onkeydown: move |event| {
                        let far = event.modifiers().shift();
                        let from = at;
                        let next = match event.key() {
                            Key::ArrowLeft => step(from, false, far),
                            Key::ArrowRight => step(from, true, far),
                            Key::Home => step(lo, false, false),
                            Key::End => step(last_index, true, false),
                            Key::Escape => None,
                            _ => return,
                        };
                        event.prevent_default();
                        cursor.set(next);
                    },
                }
            }
            SignalsReadout { columns, rows, at_time, hidden, pointed, held_take }
        }
    }
}

/// One joint's row: its key, its name, and its reading on every arm.
#[derive(Clone, Debug, PartialEq)]
struct ReadoutRow {
    name: String,
    label: String,
    color: &'static str,
    values: Vec<Option<f32>>,
}

#[component]
fn SignalsHeading(
    window: ViewWindow,
    trimmed: bool,
    on_span: EventHandler<Span>,
    on_resume: EventHandler<()>,
) -> Element {
    let held = window.span().is_none();
    rsx! {
        header { class: "signals-heading",
            h2 {
                SignalsPanelIcon {}
                span { "Signals" }
            }
            // A fixed slot that reports only the exceptions: a window the
            // operator has parked, and a take long enough to have lost its
            // oldest samples. At rest it is empty and nothing moves.
            //
            // The parked window is reported by offering the way out of it. The
            // control is named for what it does rather than for the state it
            // appears in, because its own presence is the state — and a label
            // that only says "Held" leaves the recovery in a `title`, which a
            // touch bench and a screen reader both never see.
            span { class: "signals-note",
                if held {
                    button {
                        r#type: "button",
                        class: "signals-follow",
                        title: "Return to following the newest sample",
                        onclick: move |_| on_resume.call(()),
                        "Follow"
                    }
                } else if trimmed {
                    span {
                        class: "chip chip-caution",
                        title: "This take is longer than the 30 minutes the plots keep; its earliest samples are no longer plotted. The recorded file is complete.",
                        "Trimmed"
                    }
                }
            }
            div { class: "signals-spans", role: "group", "aria-label": "Time window",
                for choice in Span::ALL {
                    button {
                        key: "{choice.label()}",
                        r#type: "button",
                        class: if window.span() == Some(choice) { "signals-span signals-span-on" } else { "signals-span" },
                        "aria-pressed": window.span() == Some(choice),
                        onclick: move |_| on_span.call(choice),
                        "{choice.label()}"
                    }
                }
            }
        }
    }
}

#[component]
fn SignalsReadout(
    columns: Vec<(String, &'static str)>,
    rows: Vec<ReadoutRow>,
    at_time: Option<f32>,
    hidden: Signal<Vec<String>>,
    pointed: Signal<Option<String>>,
    /// Set once the take has stopped, so the plots say whose history they are.
    held_take: Option<String>,
) -> Element {
    let mut hidden = hidden;
    let mut pointed = pointed;
    if rows.is_empty() {
        return rsx! {
            p { class: "signals-empty", "This take recorded no calibrated joint angles." }
        };
    }
    // Fixed value columns, wide enough for the widest angle a calibrated joint
    // can report. Sizing them to their content would let a joint crossing
    // −100° walk every joint name beside it sideways, twenty times a second.
    let template = format!(
        "grid-template-columns: minmax(0, 1fr) repeat({}, 3.25rem)",
        columns.len().max(1)
    );

    rsx! {
        div { class: "signals-readout", style: "{template}",
            div {
                class: "signals-corner",
                "aria-label": match at_time {
                    Some(seconds) => format!("Readings at {seconds:.1} seconds into the take"),
                    None => "No sample to read".to_owned(),
                },
                match at_time {
                    Some(seconds) => rsx! {
                        span { class: "signals-at", "{seconds:.1}" }
                        span { class: "signals-unit", "s" }
                    },
                    None => rsx! { span { class: "signals-unit", "—" } },
                }
            }
            for (arm , accent) in columns.iter() {
                div { key: "head-{arm}", class: "signals-column",
                    span { class: "signals-arm-dot", style: "background: {accent}" }
                    span { class: "signals-arm-name", "{arm}" }
                }
            }
            for row in rows.iter() {
                {
                    let off = hidden.read().contains(&row.name);
                    let name = row.name.clone();
                    rsx! {
                        button {
                            key: "joint-{row.name}",
                            r#type: "button",
                            class: if off { "signals-joint signals-joint-off" } else { "signals-joint" },
                            "aria-pressed": !off,
                            title: if off { "Plot this joint again" } else { "Hide this joint on every plot" },
                            // Pointing at a row lifts the trace it names on
                            // every plot, which is what makes this a legend
                            // rather than a colour-coded table. Focus counts:
                            // a keyboard walking the rows gets the same answer.
                            onmouseenter: {
                                let name = row.name.clone();
                                move |_| pointed.set(Some(name.clone()))
                            },
                            onfocus: {
                                let name = row.name.clone();
                                move |_| pointed.set(Some(name.clone()))
                            },
                            onmouseleave: move |_| pointed.set(None),
                            onblur: move |_| pointed.set(None),
                            onclick: move |_| {
                                let mut list = hidden.write();
                                match list.iter().position(|held| held == &name) {
                                    Some(index) => { list.remove(index); }
                                    None => list.push(name.clone()),
                                }
                            },
                            span { class: "signals-key-mark", style: "background: {row.color}" }
                            span { class: "signals-joint-name", "{row.label}" }
                        }
                        // The grid puts a value under its arm's heading, which
                        // a screen reader reading in order cannot see. Each
                        // cell therefore names the arm and the joint it belongs
                        // to, so the numbers survive being read one at a time.
                        for (column , reading) in columns.iter().zip(row.values.iter()) {
                            span {
                                key: "value-{row.name}-{column.0}",
                                class: if off { "signals-value signals-value-off" } else { "signals-value" },
                                "aria-label": match reading {
                                    Some(degrees) => format!("{} {}: {degrees:+.1} degrees", column.0, row.label),
                                    None => format!("{} {}: no reading", column.0, row.label),
                                },
                                match reading {
                                    Some(degrees) => rsx! {
                                        "{degrees:+.1}"
                                        span { class: "signals-unit", "°" }
                                    },
                                    None => rsx! { span { class: "signals-unit", "—" } },
                                }
                            }
                        }
                    }
                }
            }
            if let Some(take) = held_take {
                p { class: "signals-foot", "Take {take} · Record captures a new one" }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rerobot_core::api::{JointTelemetry, RecordingProgress, RobotTelemetry};

    fn joint(name: &str, degrees: Option<f64>) -> JointTelemetry {
        JointTelemetry {
            name: name.to_owned(),
            id: 1,
            raw: 0,
            degrees,
            normalized: None,
            goal_raw: None,
            target_raw: None,
            target_degrees: None,
            range: None,
            temperature_celsius: None,
            voltage_volts: None,
            current_amps: None,
            load: None,
            speed_degrees_per_second: None,
            temperature_rise_per_minute: None,
            faults: Vec::new(),
        }
    }

    fn frame(
        episode: Option<&str>,
        elapsed: f64,
        arms: Vec<(&str, Vec<JointTelemetry>)>,
    ) -> TelemetryFrame {
        TelemetryFrame {
            sequence: 0,
            loop_hz: 60.0,
            target_hz: 60.0,
            robots: arms
                .into_iter()
                .map(|(id, joints)| RobotTelemetry {
                    id: id.to_owned(),
                    torque: false,
                    joints,
                    error: None,
                    calibration: None,
                })
                .collect(),
            recording: episode.map(|episode| RecordingProgress {
                episode: episode.to_owned(),
                frames: 1,
                elapsed_seconds: elapsed,
                arms: Vec::new(),
                max_follow_error_degrees: None,
                error: None,
            }),
            replay: None,
        }
    }

    #[test]
    fn nothing_is_collected_until_a_take_is_running() {
        let mut store = SignalStore::default();
        store.observe(&frame(
            None,
            0.0,
            vec![("leader", vec![joint("gripper", Some(4.0))])],
        ));

        assert!(store.is_empty(), "an idle arm is not a recording");
        assert!(!store.live());
    }

    #[test]
    fn a_finished_take_stays_up_and_the_next_one_replaces_it() {
        let mut store = SignalStore::default();
        store.observe(&frame(
            Some("episode_000001"),
            0.0,
            vec![("leader", vec![joint("gripper", Some(1.0))])],
        ));
        store.observe(&frame(
            Some("episode_000001"),
            0.05,
            vec![("leader", vec![joint("gripper", Some(2.0))])],
        ));
        assert_eq!(store.len(), 2);

        // A stopped recorder publishes idle frames from here on; two is what it
        // takes to tell a stop from a dropped frame.
        store.observe(&frame(None, 0.0, vec![]));
        store.observe(&frame(None, 0.0, vec![]));
        assert_eq!(store.len(), 2, "a finished take stays up to be read");
        assert!(!store.live());

        store.observe(&frame(
            Some("episode_000002"),
            0.0,
            vec![("leader", vec![joint("gripper", Some(9.0))])],
        ));
        assert_eq!(store.len(), 1);
        assert_eq!(store.take(), Some("episode_000002"));
    }

    #[test]
    fn a_restarted_server_does_not_append_into_the_previous_take() {
        let mut store = SignalStore::default();
        for sample in 0..4 {
            store.observe(&frame(
                Some("episode_000001"),
                f64::from(sample),
                vec![("leader", vec![joint("gripper", Some(1.0))])],
            ));
        }
        // The server goes away and comes back numbering from one again. The id
        // is the same, so only the gap in recording and the rewound clock say
        // this is a different take.
        store.observe(&frame(None, 0.0, vec![]));
        store.observe(&frame(
            Some("episode_000001"),
            0.0,
            vec![("leader", vec![joint("gripper", Some(9.0))])],
        ));

        assert_eq!(store.len(), 1, "a fresh take replaces, never appends");
        assert!(
            store.times().windows(2).all(|pair| pair[0] <= pair[1]),
            "the time column stays sorted, or both binary searches over it lie"
        );
    }

    #[test]
    fn one_frame_without_a_take_does_not_wipe_the_one_in_progress() {
        let mut store = SignalStore::default();
        for sample in 0..4 {
            store.observe(&frame(
                Some("t"),
                f64::from(sample),
                vec![("leader", vec![joint("gripper", Some(1.0))])],
            ));
        }
        // A single frame that reports no take is a race between the recorder's
        // state and the frame carrying it, not the end of the take.
        store.observe(&frame(None, 0.0, vec![]));
        assert!(store.live(), "one dropped frame is not a stop");
        store.observe(&frame(
            Some("t"),
            4.0,
            vec![("leader", vec![joint("gripper", Some(1.0))])],
        ));
        assert_eq!(store.len(), 5, "the take continues rather than restarting");

        // Two in a row is a stop, and then the next take replaces this one.
        store.observe(&frame(None, 0.0, vec![]));
        store.observe(&frame(None, 0.0, vec![]));
        assert!(!store.live());
    }

    #[test]
    fn a_rewound_clock_starts_a_new_take_even_without_a_gap() {
        let mut store = SignalStore::default();
        store.observe(&frame(
            Some("t"),
            9.0,
            vec![("leader", vec![joint("gripper", Some(1.0))])],
        ));
        store.observe(&frame(
            Some("t"),
            0.1,
            vec![("leader", vec![joint("gripper", Some(2.0))])],
        ));

        assert_eq!(store.len(), 1);
        assert_eq!(store.times(), [0.1]);
    }

    #[test]
    fn an_arm_arriving_mid_take_is_padded_rather_than_backdated() {
        let mut store = SignalStore::default();
        store.observe(&frame(
            Some("t"),
            0.0,
            vec![("leader", vec![joint("gripper", Some(1.0))])],
        ));
        store.observe(&frame(
            Some("t"),
            0.05,
            vec![
                ("leader", vec![joint("gripper", Some(2.0))]),
                ("follower", vec![joint("gripper", Some(7.0))]),
            ],
        ));

        assert_eq!(store.reading("follower", "gripper", 0), None);
        assert_eq!(store.reading("follower", "gripper", 1), Some(7.0));
        for arm in store.arms() {
            for joint in &arm.joints {
                assert_eq!(joint.values.len(), store.len());
            }
        }
    }

    #[test]
    fn a_joint_that_stops_reporting_leaves_a_hole_not_a_shifted_series() {
        let mut store = SignalStore::default();
        store.observe(&frame(
            Some("t"),
            0.0,
            vec![(
                "leader",
                vec![joint("gripper", Some(1.0)), joint("wrist_roll", Some(3.0))],
            )],
        ));
        store.observe(&frame(
            Some("t"),
            0.05,
            vec![("leader", vec![joint("gripper", Some(2.0))])],
        ));

        assert_eq!(store.reading("leader", "wrist_roll", 0), Some(3.0));
        assert_eq!(store.reading("leader", "wrist_roll", 1), None);
        assert_eq!(store.reading("leader", "gripper", 1), Some(2.0));
    }

    #[test]
    fn an_uncalibrated_arm_records_no_angles_at_all() {
        let mut store = SignalStore::default();
        store.observe(&frame(
            Some("t"),
            0.0,
            vec![("leader", vec![joint("gripper", None)])],
        ));

        assert_eq!(store.reading("leader", "gripper", 0), None);
        assert_eq!(shared_bounds(&store, 0, 1, &[]), None);
    }

    #[test]
    fn a_long_take_trims_from_the_front_and_says_so() {
        let mut store = SignalStore::default();
        for sample in 0..MAX_SAMPLES + 1 {
            store.observe(&frame(
                Some("t"),
                sample as f64 * 0.05,
                vec![("leader", vec![joint("gripper", Some(sample as f64))])],
            ));
        }

        assert_eq!(store.len(), MAX_SAMPLES + 1 - TRIM_CHUNK);
        assert_eq!(store.trimmed(), TRIM_CHUNK);
        assert_eq!(store.arms()[0].joints[0].values.len(), store.len());
        assert_eq!(
            store.reading("leader", "gripper", 0),
            Some(TRIM_CHUNK as f32)
        );
    }

    #[test]
    fn decimation_keeps_the_peaks_an_average_would_flatten() {
        let times: Vec<f32> = (0..1_000).map(|index| index as f32 * 0.05).collect();
        let mut values: Vec<Option<f32>> = vec![Some(0.0); 1_000];
        values[500] = Some(90.0);
        values[501] = Some(-90.0);

        let reduced = decimate(&times, &values, 0, 1_000, 40);
        assert!(reduced.len() <= 60, "budget honoured: {}", reduced.len());
        let highest = reduced
            .iter()
            .filter_map(|(_, value)| *value)
            .fold(f32::NEG_INFINITY, f32::max);
        let lowest = reduced
            .iter()
            .filter_map(|(_, value)| *value)
            .fold(f32::INFINITY, f32::min);
        assert_eq!(highest, 90.0);
        assert_eq!(lowest, -90.0);
        assert!(
            reduced.windows(2).all(|pair| pair[0].0 <= pair[1].0),
            "the line still runs forwards in time"
        );
    }

    #[test]
    fn a_small_window_is_passed_through_untouched() {
        let times: Vec<f32> = (0..10).map(|index| index as f32).collect();
        let values: Vec<Option<f32>> = (0..10).map(|index| Some(index as f32)).collect();

        let reduced = decimate(&times, &values, 0, 10, POINT_BUDGET);
        assert_eq!(reduced.len(), 10);
        assert_eq!(reduced[3], (3.0, Some(3.0)));
    }

    #[test]
    fn a_dropout_survives_being_zoomed_out() {
        let times: Vec<f32> = (0..1_000).map(|index| index as f32 * 0.05).collect();
        let mut values: Vec<Option<f32>> = vec![Some(1.0); 1_000];
        for value in values.iter_mut().take(520).skip(480) {
            *value = None;
        }

        let reduced = decimate(&times, &values, 0, 1_000, 40);
        assert!(
            reduced.iter().any(|(_, value)| value.is_none()),
            "a gap must not be bridged by a line nobody measured"
        );
        // The hole is stamped where the dropout began, not at the end of the
        // bucket that noticed it — and adding it must not walk the series
        // backwards in time.
        let hole = reduced
            .iter()
            .find(|(_, value)| value.is_none())
            .expect("a hole");
        assert!(hole.0 >= 24.0 && hole.0 <= 26.0, "hole at {}", hole.0);
        assert!(
            reduced.windows(2).all(|pair| pair[0].0 <= pair[1].0),
            "still forwards in time with a hole in the bucket"
        );
    }

    #[test]
    fn the_window_follows_the_newest_sample_until_it_is_held() {
        let following = ViewWindow::Following(Span::Recent);
        assert_eq!(following.resolve((0.0, 30.0)), (20.0, 30.0));
        // A take shorter than the window shows the take, not empty axis.
        assert_eq!(following.resolve((0.0, 4.0)), (0.0, 4.0));
        assert_eq!(
            ViewWindow::Following(Span::All).resolve((2.0, 40.0)),
            (2.0, 40.0)
        );

        let held = ViewWindow::Held {
            start: 5.0,
            end: 9.0,
        };
        assert_eq!(held.resolve((0.0, 100.0)), (5.0, 9.0));
        assert_eq!(held.span(), None, "a held window matches no preset");
    }

    #[test]
    fn the_window_selects_a_sample_range_with_overhang() {
        let mut store = SignalStore::default();
        for sample in 0..10 {
            store.observe(&frame(
                Some("t"),
                sample as f64,
                vec![("leader", vec![joint("gripper", Some(sample as f64))])],
            ));
        }

        let (lo, hi) = store.range(3.0, 6.0);
        assert!(lo <= 2, "one sample of overhang so a line enters the window");
        assert!(hi >= 7);
        assert_eq!(store.nearest(5.4), Some(5));
        assert_eq!(store.nearest(-2.0), Some(0));
        assert_eq!(store.nearest(99.0), Some(9));
    }

    #[test]
    fn every_plot_lands_on_the_same_gridlines() {
        assert_eq!(nice_range(0.0, 0.0, 4), (-1.0, 1.0, 1.0));

        // The step has to survive the rounding it causes: this range picked a
        // 60° step on its measured span and then drew five gaps once the axis
        // was widened to -180..120.
        let (low, high, step) = nice_range(-170.0, 70.0, 4);
        assert!(low <= -170.0 && high >= 70.0);
        assert!((high - low) / step <= 4.0, "{low}..{high} by {step}");

        for divisions in 2..=5 {
            for (low, high) in [(-12.0, 47.0), (-100.0, 100.0), (-3.5, 0.25), (0.0, 179.0)] {
                let (axis_low, axis_high, step) = nice_range(low, high, divisions);
                assert!(axis_low <= low && axis_high >= high, "{low}..{high}");
                assert!(
                    (axis_high - axis_low) / step <= divisions as f32,
                    "{low}..{high} by {divisions}"
                );
            }
        }
    }

    #[test]
    fn joints_keep_their_colour_when_a_motor_is_missing() {
        // Dropping shoulder_lift must not repaint elbow_flex, which is exactly
        // what colouring by list position would do.
        assert_eq!(signal_color("shoulder_pan"), SIGNAL_COLORS[0]);
        assert_eq!(signal_color("elbow_flex"), SIGNAL_COLORS[2]);
        assert_eq!(signal_color("gripper"), SIGNAL_COLORS[5]);
        assert_eq!(signal_color("extra_axis"), signal_color("extra_axis"));
        assert_eq!(joint_label("wrist_roll"), "Wrist roll");
        assert_eq!(joint_label("extra_axis"), "extra_axis");
    }

    #[test]
    fn the_time_axis_is_labelled_in_round_seconds_only() {
        // A window that starts wherever the live edge left it must not print
        // its own ragged edge as a tick.
        let ticks = time_ticks(180.31, 190.31, 5);
        assert_eq!(ticks, vec![182.0, 184.0, 186.0, 188.0, 190.0]);
        assert!(ticks.iter().all(|tick| *tick >= 180.31 && *tick <= 190.31));

        // And the step grows with the window rather than the label count.
        assert!(time_ticks(0.0, 1_800.0, 5).len() <= 5);
        assert!(time_ticks(0.0, 0.4, 5).len() >= 2);
        // A window with no round second in it gets no labels rather than a
        // fabricated one.
        assert!(time_ticks(10.02, 10.04, 1).is_empty());
    }

    #[test]
    fn hidden_joints_do_not_hold_the_shared_scale_open() {
        let mut store = SignalStore::default();
        store.observe(&frame(
            Some("t"),
            0.0,
            vec![(
                "leader",
                vec![joint("gripper", Some(90.0)), joint("elbow_flex", Some(3.0))],
            )],
        ));

        assert_eq!(shared_bounds(&store, 0, 1, &[]), Some((3.0, 90.0)));
        assert_eq!(
            shared_bounds(&store, 0, 1, &["gripper".to_owned()]),
            Some((3.0, 3.0)),
            "a hidden joint must not keep every other trace squashed"
        );
    }
}
