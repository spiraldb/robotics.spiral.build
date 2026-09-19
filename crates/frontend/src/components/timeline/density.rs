//! Message density rendered in the shared timeline coordinates.
use super::TimelineState;
use dioxus::prelude::*;

#[component]
pub(crate) fn DensityTrack(buckets: Vec<u32>, state: TimelineState, title: String) -> Element {
    let view = state.window();
    let count = buckets.len() as f64;
    let duration = state.clock().duration();
    let scale = if duration > 0.0 {
        count / duration
    } else {
        count
    };
    let x0 = view.start() * scale;
    let width = (view.span() * scale).max(1e-6);
    let tiers = buckets.density_paths();
    let gaps = buckets.gap_path();
    rsx! { div { class: "data-track", title,
        svg { class: "data-density", view_box: "{x0} 0 {width} 1", preserve_aspect_ratio: "none", "aria-hidden": "true",
            for (index, path) in tiers.iter().enumerate() { path { class: "data-density-{index + 1}", d: "{path}" } }
            path { class: "data-density-gap", d: "{gaps}" }
        }
    } }
}

/// Runs of equally dense buckets become one bar each, so a steady 30 Hz
/// stream draws as a solid band rather than 240 seams.
trait DensityPaths {
    fn density_paths(&self) -> [String; 3];
    /// Empty buckets between the first and last message of a dense stream:
    /// a dropout worth a glance. A sparse stream has spacing, not gaps, and
    /// draws none.
    fn gap_path(&self) -> String;
}

impl DensityPaths for [u32] {
    fn gap_path(&self) -> String {
        let Some(first) = self.iter().position(|count| *count > 0) else {
            return String::new();
        };
        let last = self.iter().rposition(|count| *count > 0).unwrap_or(first);
        let span = &self[first..=last];
        let occupied = span.iter().filter(|count| **count > 0).count();
        if occupied * 2 < span.len() {
            return String::new();
        }
        let mut path = String::new();
        let mut run: Option<(usize, usize)> = None;
        for (index, count) in span.iter().enumerate() {
            match (run, *count == 0) {
                (Some((start, length)), true) => run = Some((start, length + 1)),
                (Some((start, length)), false) => {
                    path.push_str(&format!("M{} .42h{length}v.16h-{length}z", first + start));
                    run = None;
                }
                (None, true) => run = Some((index, 1)),
                (None, false) => {}
            }
        }
        path
    }
    fn density_paths(&self) -> [String; 3] {
        // Tiers compare against the stream's typical occupied bucket, not its
        // peak: a 30 Hz stream over 240 buckets alternates between 2 and 3
        // messages per bucket (with the odd 4), and must read as one solid
        // band rather than a moiré of alternating tints.
        let occupied = self.iter().filter(|count| **count > 0).count().max(1) as f64;
        let typical = self.iter().map(|count| f64::from(*count)).sum::<f64>() / occupied;
        let tier_of = |count: u32| -> Option<usize> {
            if count == 0 {
                return None;
            }
            let share = f64::from(count) / typical.max(1.0);
            Some(if share >= 0.6 {
                2
            } else if share >= 0.3 {
                1
            } else {
                0
            })
        };
        let mut paths = [String::new(), String::new(), String::new()];
        let mut run: Option<(usize, usize, usize)> = None;
        for (index, count) in self.iter().enumerate() {
            let tier = tier_of(*count);
            match (run, tier) {
                (Some((start, length, current)), Some(tier)) if tier == current => {
                    run = Some((start, length + 1, current));
                }
                _ => {
                    if let Some((start, length, current)) = run.take() {
                        paths[current].push_str(&format!("M{start} .2h{length}v.6h-{length}z"));
                    }
                    run = tier.map(|tier| (index, 1, tier));
                }
            }
        }
        if let Some((start, length, current)) = run {
            paths[current].push_str(&format!("M{start} .2h{length}v.6h-{length}z"));
        }
        paths
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn density_runs_merge_into_single_bars_per_tier() {
        // Typical occupied bucket is 7.2: 10s are dense, 4 is medium, 2 is sparse.
        let paths = [10_u32, 10, 2, 0, 0, 4, 10].density_paths();
        assert_eq!(paths[2], "M0 .2h2v.6h-2zM6 .2h1v.6h-1z");
        assert_eq!(paths[0], "M2 .2h1v.6h-1z");
        assert_eq!(paths[1], "M5 .2h1v.6h-1z");
        let steady = [3_u32, 2, 3, 4, 2, 3].density_paths();
        assert_eq!(steady[2], "M0 .2h6v.6h-6z");
        assert_eq!([1_u32].density_paths()[2], "M0 .2h1v.6h-1z");
        assert!([0_u32, 0].density_paths().iter().all(String::is_empty));
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn only_dense_streams_report_gaps_and_only_between_their_messages() {
        // A dropout inside a steady stream is a gap; leading and trailing
        // silence is not.
        assert_eq!([0_u32, 3, 3, 0, 0, 3, 3, 0].gap_path(), "M3 .42h2v.16h-2z");
        assert_eq!(
            [3_u32, 0, 3, 3, 0, 3].gap_path(),
            "M1 .42h1v.16h-1zM4 .42h1v.16h-1z"
        );
        // A sparse stream is spacing, not gaps; a single message has neither.
        assert_eq!([1_u32, 0, 0, 0, 0, 0, 0, 1].gap_path(), "");
        assert_eq!([0_u32, 1, 0].gap_path(), "");
        assert_eq!([0_u32, 0].gap_path(), "");
    }
}
