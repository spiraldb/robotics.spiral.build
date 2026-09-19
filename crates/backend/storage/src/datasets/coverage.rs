//! Bucketed message counts for the streams of one episode.
//!
//! The Data timeline asks for every stream at once; each stream's query
//! projects only `log_time` and counts rows as they stream, so no message
//! payloads are read.
use super::messages::MessageQuery;
use crate::{
    StorageManager,
    db::{ArrowColumns, DatabaseSession},
};
use futures::StreamExt as _;
use spiral_robotics_common::datasets::{
    EpisodeCoverageRequest, StreamCoverage, TopicCoverage, TopicCoverageRequest,
};

/// Streams counted at the same time for one request; the rest queue. Each
/// count is one small query, so an episode's two dozen streams answer in a
/// few rounds rather than a few seconds each.
const CONCURRENT_COVERAGE_QUERIES: usize = 8;

impl StorageManager {
    /// Every stream of an episode, a few queries at a time, answered in
    /// request order. The batch belongs to one dataset revision, so the
    /// catalog is inspected once before the counts and checked once after
    /// them rather than around every stream. One stream that cannot be
    /// counted reports its error in place instead of failing the others.
    pub async fn episode_coverage(
        &self,
        request: EpisodeCoverageRequest,
    ) -> Result<Vec<StreamCoverage>, String> {
        request.validate()?;
        let Some(first) = request.streams().first() else {
            return Ok(Vec::new());
        };
        let dataset = first.read().topic().dataset();
        if request
            .streams()
            .iter()
            .any(|stream| stream.read().topic().dataset() != dataset)
        {
            return Err("Coverage covers the streams of one dataset per request".into());
        }
        let inspection = self
            .project_store()
            .dataset_catalog()
            .inspect(dataset.clone())
            .await?;
        let outcomes = futures::stream::iter(request.streams().iter().cloned())
            .map(|stream| {
                let known = Self::revision_known(&inspection, stream.read());
                async move {
                    let counted = match known {
                        Ok(()) => self.count_log_times(&stream).await,
                        Err(error) => Err(error),
                    };
                    match counted {
                        Ok(coverage) => StreamCoverage::Ready { coverage },
                        Err(error) => StreamCoverage::Failed { error },
                    }
                }
            })
            .buffered(CONCURRENT_COVERAGE_QUERIES)
            .collect()
            .await;
        self.require_same_revision_after(first.read()).await?;
        Ok(outcomes)
    }

    /// Bucket one stream's `log_time`s; the caller has already checked the
    /// dataset revision.
    async fn count_log_times(
        &self,
        request: &TopicCoverageRequest,
    ) -> Result<TopicCoverage, String> {
        let bounds = request.bounds()?;
        let statement = MessageQuery::new(request.read()).statement(
            "log_time",
            &format!("AND log_time >= {} AND log_time < {}", bounds.0, bounds.1),
        )?;
        let mut connection = self.project_store().connect().await?;
        let mut stream = connection
            .query(statement)
            .await
            .map_err(|error| error.to_string())?;
        let mut coverage = TopicCoverage::empty(request.buckets());
        while let Some(batch) = stream.next().await {
            let batch = batch.map_err(|error| error.to_string())?;
            let columns = ArrowColumns::new(&batch, "topic coverage");
            for row in 0..batch.num_rows() {
                if let Some(log_time) = columns.optional_unsigned("log_time", row)? {
                    coverage.observe(log_time, bounds);
                }
            }
        }
        Ok(coverage)
    }
}
