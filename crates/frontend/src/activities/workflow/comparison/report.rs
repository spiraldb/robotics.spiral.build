use serde::Deserialize;
use spiral_robotics_common::{
    relations::SpiralQuerySnapshot, workflow::comparison::ComparisonBaseline,
};

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub(super) struct ComparisonBin {
    metric: i64,
    bucket: i64,
    low: Option<f64>,
    high: Option<f64>,
    #[serde(flatten)]
    counts: PopulationCounts,
    #[serde(flatten)]
    sums: PopulationSums,
    invalid_count: u64,
}
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
struct PopulationCounts {
    all_n: u64,
    selected_n: u64,
    remaining_n: u64,
    unknown_n: u64,
}
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
struct PopulationSums {
    all_sum: Option<f64>,
    selected_sum: Option<f64>,
    remaining_sum: Option<f64>,
}
impl ComparisonBin {
    pub(super) fn selected(&self) -> u64 {
        self.counts.selected_n
    }
    pub(super) fn remaining(&self) -> u64 {
        self.counts.remaining_n
    }
    pub(super) fn unknown(&self) -> u64 {
        self.counts.unknown_n
    }
    pub(super) fn all(&self) -> u64 {
        self.counts.all_n
    }
    pub(super) fn baseline(&self, baseline: ComparisonBaseline) -> u64 {
        match baseline {
            ComparisonBaseline::All => self.all(),
            ComparisonBaseline::Remaining => self.remaining(),
        }
    }
    pub(super) fn hours(&self) -> f64 {
        self.sums.all_sum.unwrap_or_default() / 3600.0
    }
    pub(super) fn selected_hours(&self) -> f64 {
        self.sums.selected_sum.unwrap_or_default() / 3600.0
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
            return Err("Comparison aggregates were truncated. The dashboard cannot display incomplete counts.".into());
        }
        let rows = snapshot
            .rows()
            .iter()
            .map(|row| {
                serde_json::from_value::<ComparisonBin>(serde_json::Value::Object(row.clone()))
                    .map_err(|e| format!("Reading comparison aggregates: {e}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let population = rows
            .iter()
            .find(|r| r.metric == -1)
            .cloned()
            .ok_or("Spiral returned no population summary.")?;
        if population.invalid_count != 0 {
            return Err("The measurement query returns multiple rows per episode. Group measurements by episode_id before comparing.".into());
        }
        if population.all() != population.selected() + population.remaining() + population.unknown()
        {
            return Err("Episode counts do not reconcile.".into());
        }
        let metrics = (0..metric_count)
            .map(|index| {
                MetricDistribution::new(
                    rows.iter()
                        .filter(|r| r.metric == index as i64)
                        .cloned()
                        .collect(),
                )
            })
            .collect();
        Ok(Self {
            population,
            metrics,
        })
    }
    pub(super) fn population(&self) -> &ComparisonBin {
        &self.population
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
    fn new(bins: Vec<ComparisonBin>) -> Self {
        Self { bins }
    }
    pub(super) fn with_selected_baseline(&self, baseline: &Self) -> Result<Self, String> {
        if self.range() != baseline.range() {
            return Err("These outputs use different distribution ranges. Comparing them is not supported yet.".into());
        }
        let mut bins = Vec::new();
        for bucket in 0..10 {
            let selected = self.bins.iter().find(|bin| bin.bucket() == bucket);
            let reference = baseline.bins.iter().find(|bin| bin.bucket() == bucket);
            if let Some(template) = selected.or(reference) {
                let mut bin = template.clone();
                bin.counts.selected_n = selected.map_or(0, ComparisonBin::selected);
                bin.sums.selected_sum = selected.and_then(|row| row.sums.selected_sum);
                bin.counts.all_n = reference.map_or(0, ComparisonBin::selected);
                bin.sums.all_sum = reference.and_then(|row| row.sums.selected_sum);
                bin.counts.remaining_n = 0;
                bin.counts.unknown_n = 0;
                bin.sums.remaining_sum = None;
                bins.push(bin);
            }
        }
        Ok(Self::new(bins))
    }
    pub(super) fn bins(&self) -> &[ComparisonBin] {
        &self.bins
    }
    pub(super) fn range(&self) -> (f64, f64) {
        self.bins
            .first()
            .map(|bin| (bin.low.unwrap_or_default(), bin.high.unwrap_or_default()))
            .unwrap_or_default()
    }
    pub(super) fn selected_count(&self) -> u64 {
        self.bins.iter().map(ComparisonBin::selected).sum()
    }
    pub(super) fn baseline_count(&self, baseline: ComparisonBaseline) -> u64 {
        self.bins.iter().map(|bin| bin.baseline(baseline)).sum()
    }
    pub(super) fn selected_average(&self) -> Option<f64> {
        let n = self.selected_count();
        (n > 0).then(|| {
            self.bins
                .iter()
                .filter_map(|b| b.sums.selected_sum)
                .sum::<f64>()
                / n as f64
        })
    }
    pub(super) fn baseline_average(&self, baseline: ComparisonBaseline) -> Option<f64> {
        let n = self.baseline_count(baseline);
        (n > 0).then(|| {
            self.bins
                .iter()
                .filter_map(|b| match baseline {
                    ComparisonBaseline::All => b.sums.all_sum,
                    ComparisonBaseline::Remaining => b.sums.remaining_sum,
                })
                .sum::<f64>()
                / n as f64
        })
    }
    pub(super) fn delta(&self, baseline: ComparisonBaseline) -> String {
        match (self.selected_average(), self.baseline_average(baseline)) {
            (Some(selected), Some(base)) => {
                let difference = selected - base;
                let sign = if difference > 0.0 { "+" } else { "" };
                format!("{sign}{}", MetricNumber(Some(difference)).display())
            }
            _ => "—".into(),
        }
    }
    pub(super) fn height(&self, bin: usize, baseline: ComparisonBaseline) -> (f64, f64) {
        let selected_total = self.selected_count().max(1) as f64;
        let baseline_total = self.baseline_count(baseline).max(1) as f64;
        let peak = self
            .bins
            .iter()
            .map(|b| {
                (b.selected() as f64 / selected_total)
                    .max(b.baseline(baseline) as f64 / baseline_total)
            })
            .fold(0.0_f64, f64::max)
            .max(f64::EPSILON);
        self.bins
            .iter()
            .find(|b| b.bucket() == bin)
            .map(|b| {
                (
                    100.0 * b.selected() as f64 / selected_total / peak,
                    100.0 * b.baseline(baseline) as f64 / baseline_total / peak,
                )
            })
            .unwrap_or_default()
    }
    pub(super) fn bin_label(&self, bin: usize) -> String {
        let (low, high) = self.range();
        let width = (high - low) / 10.0;
        format!(
            "{}–{}",
            MetricNumber(Some(low + width * bin as f64)).display(),
            MetricNumber(Some(low + width * (bin + 1) as f64)).display()
        )
    }
}
pub(super) struct MetricNumber(pub(super) Option<f64>);
impl MetricNumber {
    pub(super) fn display(&self) -> String {
        match self.0.filter(|value| value.is_finite()) {
            None => "—".into(),
            Some(value) if value.abs() >= 1000.0 => format!("{value:.0}"),
            Some(value) if value.abs() >= 10.0 => format!("{value:.1}"),
            Some(value) => format!("{value:.3}")
                .trim_end_matches('0')
                .trim_end_matches('.')
                .to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Fixture;
    impl Fixture {
        fn snapshot() -> SpiralQuerySnapshot {
            let rows = vec![
                json!({"metric":-1,"bucket":-1,"low":null,"high":null,"all_n":5,"selected_n":2,"remaining_n":2,"unknown_n":1,"all_sum":100.0,"selected_sum":30.0,"remaining_sum":60.0,"invalid_count":0}),
                json!({"metric":0,"bucket":0,"low":0.0,"high":10.0,"all_n":3,"selected_n":1,"remaining_n":1,"unknown_n":1,"all_sum":6.0,"selected_sum":1.0,"remaining_sum":3.0,"invalid_count":0}),
                json!({"metric":0,"bucket":9,"low":0.0,"high":10.0,"all_n":1,"selected_n":1,"remaining_n":0,"unknown_n":0,"all_sum":10.0,"selected_sum":10.0,"remaining_sum":null,"invalid_count":0}),
            ];
            SpiralQuerySnapshot::new(
                vec![],
                rows.into_iter()
                    .map(|r| r.as_object().unwrap().clone())
                    .collect(),
                false,
            )
        }
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn uses_measured_counts_for_weighted_means_and_keeps_unknowns_in_all_baseline() {
        let report = ComparisonReport::from_snapshot(&Fixture::snapshot(), 2).unwrap();
        let metric = &report.metrics()[0];
        assert_eq!(metric.selected_average(), Some(5.5));
        assert_eq!(metric.baseline_average(ComparisonBaseline::All), Some(4.0));
        assert_eq!(
            metric.baseline_average(ComparisonBaseline::Remaining),
            Some(3.0)
        );
        assert_eq!(metric.baseline_count(ComparisonBaseline::All), 4);
        assert_eq!(metric.delta(ComparisonBaseline::All), "+1.5");
        assert_eq!(report.population().unknown(), 1);
        assert_eq!(report.metrics()[1].selected_average(), None);
        assert_eq!(metric.height(5, ComparisonBaseline::All), (0.0, 0.0));
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn another_workflow_baseline_uses_its_selected_population() {
        let report = ComparisonReport::from_snapshot(&Fixture::snapshot(), 1).unwrap();
        let metric = &report.metrics()[0];
        let compared = metric.with_selected_baseline(metric).unwrap();
        assert_eq!(compared.baseline_count(ComparisonBaseline::All), 2);
        assert_eq!(
            compared.baseline_average(ComparisonBaseline::All),
            Some(5.5)
        );
        assert_eq!(compared.selected_average(), Some(5.5));
        let mut different = metric.clone();
        different.bins[0].high = Some(20.0);
        assert!(metric.with_selected_baseline(&different).is_err());
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn refuses_truncated_or_duplicated_populations() {
        let snapshot = Fixture::snapshot();
        let truncated = SpiralQuerySnapshot::new(vec![], snapshot.rows().to_vec(), true);
        assert!(ComparisonReport::from_snapshot(&truncated, 1).is_err());
        let mut rows = snapshot.into_rows();
        rows[0].insert("invalid_count".into(), json!(1));
        assert!(
            ComparisonReport::from_snapshot(&SpiralQuerySnapshot::new(vec![], rows, false), 1)
                .is_err()
        );
    }
}
