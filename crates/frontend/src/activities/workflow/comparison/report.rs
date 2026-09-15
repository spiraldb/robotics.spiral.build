use serde::Deserialize;
use spiral_robotics_common::relations::SpiralQuerySnapshot;

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub(super) struct ComparisonBin {
    metric: i64,
    bucket: i64,
    low: Option<f64>,
    high: Option<f64>,
    #[serde(flatten)]
    counts: PairCounts,
    #[serde(flatten)]
    sums: PairSums,
    invalid_count: u64,
}
#[derive(Clone, Debug, PartialEq, Deserialize)]
struct PairCounts {
    left_n: u64,
    right_n: u64,
}
#[derive(Clone, Debug, PartialEq, Deserialize)]
struct PairSums {
    left_sum: Option<f64>,
    right_sum: Option<f64>,
}
impl ComparisonBin {
    pub(super) fn left(&self) -> u64 {
        self.counts.left_n
    }
    pub(super) fn right(&self) -> u64 {
        self.counts.right_n
    }
    pub(super) fn bucket(&self) -> usize {
        self.bucket.max(0) as usize
    }
}
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ComparisonReport {
    population: ComparisonBin,
    metrics: Vec<MetricDistribution>,
}
impl ComparisonReport {
    pub(super) fn from_snapshot(
        snapshot: &SpiralQuerySnapshot,
        metric_count: usize,
    ) -> Result<Self, String> {
        if snapshot.truncated() {
            return Err("Comparison aggregates were truncated.".into());
        }
        let rows = snapshot
            .rows()
            .iter()
            .map(|r| {
                serde_json::from_value::<ComparisonBin>(serde_json::Value::Object(r.clone()))
                    .map_err(|e| e.to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let population = rows
            .iter()
            .find(|r| r.metric == -1)
            .cloned()
            .ok_or("No comparison counts were returned.")?;
        if population.invalid_count != 0 {
            return Err("Episode comparisons require one non-null episode_id per episode in each normalized output. Group repeated frames or moments first.".into());
        }
        let metrics = (0..metric_count)
            .map(|i| MetricDistribution {
                bins: rows
                    .iter()
                    .filter(|r| r.metric == i as i64)
                    .cloned()
                    .collect(),
            })
            .collect();
        Ok(Self {
            population,
            metrics,
        })
    }
    pub(super) fn counts(&self) -> (u64, u64) {
        (self.population.left(), self.population.right())
    }
    pub(super) fn metrics(&self) -> &[MetricDistribution] {
        &self.metrics
    }
}
#[derive(Clone, Debug, PartialEq)]
pub(super) struct MetricDistribution {
    bins: Vec<ComparisonBin>,
}
impl MetricDistribution {
    pub(super) fn counts(&self) -> (u64, u64) {
        (
            self.bins.iter().map(ComparisonBin::left).sum(),
            self.bins.iter().map(ComparisonBin::right).sum(),
        )
    }
    pub(super) fn averages(&self) -> (Option<f64>, Option<f64>) {
        let (left, right) = self.counts();
        (
            (left > 0).then(|| {
                self.bins
                    .iter()
                    .filter_map(|b| b.sums.left_sum)
                    .sum::<f64>()
                    / left as f64
            }),
            (right > 0).then(|| {
                self.bins
                    .iter()
                    .filter_map(|b| b.sums.right_sum)
                    .sum::<f64>()
                    / right as f64
            }),
        )
    }
    pub(super) fn range(&self) -> (f64, f64) {
        self.bins
            .first()
            .map(|b| (b.low.unwrap_or(0.0), b.high.unwrap_or(0.0)))
            .unwrap_or_default()
    }
    pub(super) fn bin_counts(&self, bucket: usize) -> (u64, u64) {
        self.bins
            .iter()
            .find(|b| b.bucket() == bucket)
            .map(|b| (b.left(), b.right()))
            .unwrap_or_default()
    }
    pub(super) fn heights(&self, bucket: usize) -> (f64, f64) {
        let (ln, rn) = self.counts();
        let shares = |i| {
            let (l, r) = self.bin_counts(i);
            (l as f64 / ln.max(1) as f64, r as f64 / rn.max(1) as f64)
        };
        let peak = (0..10)
            .map(|i| {
                let (l, r) = shares(i);
                l.max(r)
            })
            .fold(0.0_f64, f64::max)
            .max(f64::EPSILON);
        let (l, r) = shares(bucket);
        (l / peak * 100.0, r / peak * 100.0)
    }
    pub(super) fn bin_label(&self, bucket: usize) -> String {
        let (low, high) = self.range();
        let step = (high - low) / 10.0;
        format!(
            "{}–{}",
            MetricNumber(Some(low + step * bucket as f64)).display(),
            MetricNumber(Some(low + step * (bucket + 1) as f64)).display()
        )
    }
}
pub(super) struct MetricNumber(pub Option<f64>);
impl MetricNumber {
    pub(super) fn display(&self) -> String {
        match self.0.filter(|v| v.is_finite()) {
            None => "—".into(),
            Some(v) if v.abs() >= 1000.0 => format!("{v:.0}"),
            Some(v) if v.abs() >= 10.0 => format!("{v:.1}"),
            Some(v) => format!("{v:.3}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn paired_populations_use_independent_counts_and_measured_means() {
        let rows=vec![
            json!({"metric":-1,"bucket":-1,"low":null,"high":null,"left_n":49,"right_n":52,"left_sum":null,"right_sum":null,"invalid_count":0}),
            json!({"metric":0,"bucket":0,"low":2,"high":8,"left_n":2,"right_n":4,"left_sum":6,"right_sum":20,"invalid_count":0}),
            json!({"metric":0,"bucket":9,"low":2,"high":8,"left_n":1,"right_n":1,"left_sum":8,"right_sum":8,"invalid_count":0}),
        ].into_iter().map(|r|r.as_object().unwrap().clone()).collect();
        let snapshot = SpiralQuerySnapshot::new(vec![], rows, false);
        let report = ComparisonReport::from_snapshot(&snapshot, 2).unwrap();
        assert_eq!(report.counts(), (49, 52));
        assert_eq!(report.metrics()[0].counts(), (3, 5));
        assert_eq!(
            report.metrics()[0].averages(),
            (Some(14.0 / 3.0), Some(28.0 / 5.0))
        );
        assert_eq!(report.metrics()[1].averages(), (None, None));
        let mut duplicate = snapshot.rows().to_vec();
        duplicate[0].insert("invalid_count".into(), json!(1));
        assert!(
            ComparisonReport::from_snapshot(&SpiralQuerySnapshot::new(vec![], duplicate, false), 1)
                .is_err()
        );
        assert!(
            ComparisonReport::from_snapshot(
                &SpiralQuerySnapshot::new(vec![], snapshot.rows().to_vec(), true),
                1
            )
            .is_err()
        );
    }
}
