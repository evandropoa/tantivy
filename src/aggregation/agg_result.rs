//! Contains the final aggregation tree.
//! This tree can be converted via the `into()` method from `IntermediateAggregationResults`.
//! This conversion computes the final result. For example: The intermediate result contains
//! intermediate average results, which is the sum and the number of values. The actual average is
//! calculated on the step from intermediate to final aggregation result tree.

use std::cmp::Ordering;
use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::agg_req::{
    Aggregations, AggregationsInternal, BucketAggregationInternal, MetricAggregation,
};
use super::bucket::{intermediate_buckets_to_final_buckets, GetDocCount};
use super::intermediate_agg_result::{
    IntermediateAggregationResults, IntermediateBucketResult, IntermediateHistogramBucketEntry,
    IntermediateMetricResult, IntermediateRangeBucketEntry,
};
use super::metric::{SingleMetricResult, Stats};
use super::{Key, VecWithNames};
use crate::TantivyError;

#[derive(Clone, Default, Debug, PartialEq, Serialize, Deserialize)]
/// The final aggegation result.
pub struct AggregationResults(pub HashMap<String, AggregationResult>);

impl AggregationResults {
    pub(crate) fn get_value_from_aggregation(
        &self,
        name: &str,
        agg_property: &str,
    ) -> crate::Result<Option<f64>> {
        if let Some(agg) = self.0.get(name) {
            agg.get_value_from_aggregation(name, agg_property)
        } else {
            // Validation is be done during request parsing, so we can't reach this state.
            Err(TantivyError::InternalError(format!(
                "Can't find aggregation {:?} in sub_aggregations",
                name
            )))
        }
    }

    /// Convert and intermediate result and its aggregation request to the final result
    pub fn from_intermediate_and_req(
        results: IntermediateAggregationResults,
        agg: Aggregations,
    ) -> crate::Result<Self> {
        AggregationResults::from_intermediate_and_req_internal(results, &(agg.into()))
    }

    /// Convert and intermediate result and its aggregation request to the final result
    ///
    /// Internal function, CollectorAggregations is used instead Aggregations, which is optimized
    /// for internal processing, by splitting metric and buckets into seperate groups.
    pub(crate) fn from_intermediate_and_req_internal(
        intermediate_results: IntermediateAggregationResults,
        req: &AggregationsInternal,
    ) -> crate::Result<Self> {
        // Important assumption:
        // When the tree contains buckets/metric, we expect it to have all buckets/metrics from the
        // request
        let mut results: HashMap<String, AggregationResult> = HashMap::new();

        if let Some(buckets) = intermediate_results.buckets {
            add_coverted_final_buckets_to_result(&mut results, buckets, &req.buckets)?
        } else {
            // When there are no buckets, we create empty buckets, so that the serialized json
            // format is constant
            add_empty_final_buckets_to_result(&mut results, &req.buckets)?
        };

        if let Some(metrics) = intermediate_results.metrics {
            add_converted_final_metrics_to_result(&mut results, metrics);
        } else {
            // When there are no metrics, we create empty metric results, so that the serialized
            // json format is constant
            add_empty_final_metrics_to_result(&mut results, &req.metrics)?;
        }
        Ok(Self(results))
    }
}

fn add_converted_final_metrics_to_result(
    results: &mut HashMap<String, AggregationResult>,
    metrics: VecWithNames<IntermediateMetricResult>,
) {
    results.extend(
        metrics
            .into_iter()
            .map(|(key, metric)| (key, AggregationResult::MetricResult(metric.into()))),
    );
}

fn add_empty_final_metrics_to_result(
    results: &mut HashMap<String, AggregationResult>,
    req_metrics: &VecWithNames<MetricAggregation>,
) -> crate::Result<()> {
    results.extend(req_metrics.iter().map(|(key, req)| {
        let empty_bucket = IntermediateMetricResult::empty_from_req(req);
        (
            key.to_string(),
            AggregationResult::MetricResult(empty_bucket.into()),
        )
    }));
    Ok(())
}

fn add_empty_final_buckets_to_result(
    results: &mut HashMap<String, AggregationResult>,
    req_buckets: &VecWithNames<BucketAggregationInternal>,
) -> crate::Result<()> {
    let requested_buckets = req_buckets.iter();
    for (key, req) in requested_buckets {
        let empty_bucket = AggregationResult::BucketResult(BucketResult::empty_from_req(req)?);
        results.insert(key.to_string(), empty_bucket);
    }
    Ok(())
}

fn add_coverted_final_buckets_to_result(
    results: &mut HashMap<String, AggregationResult>,
    buckets: VecWithNames<IntermediateBucketResult>,
    req_buckets: &VecWithNames<BucketAggregationInternal>,
) -> crate::Result<()> {
    assert_eq!(buckets.len(), req_buckets.len());

    let buckets_with_request = buckets.into_iter().zip(req_buckets.values());
    for ((key, bucket), req) in buckets_with_request {
        let result =
            AggregationResult::BucketResult(BucketResult::from_intermediate_and_req(bucket, req)?);
        results.insert(key, result);
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
/// An aggregation is either a bucket or a metric.
pub enum AggregationResult {
    /// Bucket result variant.
    BucketResult(BucketResult),
    /// Metric result variant.
    MetricResult(MetricResult),
}

impl AggregationResult {
    pub(crate) fn get_value_from_aggregation(
        &self,
        _name: &str,
        agg_property: &str,
    ) -> crate::Result<Option<f64>> {
        match self {
            AggregationResult::BucketResult(_bucket) => Err(TantivyError::InternalError(
                "Tried to retrieve value from bucket aggregation. This is not supported and \
                 should not happen during collection, but should be catched during validation"
                    .to_string(),
            )),
            AggregationResult::MetricResult(metric) => metric.get_value(agg_property),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
/// MetricResult
pub enum MetricResult {
    /// Average metric result.
    Average(SingleMetricResult),
    /// Stats metric result.
    Stats(Stats),
}

impl MetricResult {
    fn get_value(&self, agg_property: &str) -> crate::Result<Option<f64>> {
        match self {
            MetricResult::Average(avg) => Ok(avg.value),
            MetricResult::Stats(stats) => stats.get_value(agg_property),
        }
    }
}
impl From<IntermediateMetricResult> for MetricResult {
    fn from(metric: IntermediateMetricResult) -> Self {
        match metric {
            IntermediateMetricResult::Average(avg_data) => {
                MetricResult::Average(avg_data.finalize().into())
            }
            IntermediateMetricResult::Stats(intermediate_stats) => {
                MetricResult::Stats(intermediate_stats.finalize())
            }
        }
    }
}

/// BucketEntry holds bucket aggregation result types.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BucketResult {
    /// This is the range entry for a bucket, which contains a key, count, from, to, and optionally
    /// sub_aggregations.
    Range {
        /// The range buckets sorted by range.
        buckets: Vec<RangeBucketEntry>,
    },
    /// This is the histogram entry for a bucket, which contains a key, count, and optionally
    /// sub_aggregations.
    Histogram {
        /// The buckets.
        ///
        /// If there are holes depends on the request, if min_doc_count is 0, then there are no
        /// holes between the first and last bucket.
        /// See [HistogramAggregation](super::bucket::HistogramAggregation)
        buckets: Vec<BucketEntry>,
    },
    /// This is the term result
    Terms {
        /// The buckets.
        ///
        /// See [TermsAggregation](super::bucket::TermsAggregation)
        buckets: Vec<BucketEntry>,
        /// The number of documents that didn’t make it into to TOP N due to shard_size or size
        sum_other_doc_count: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        /// The upper bound error for the doc count of each term.
        doc_count_error_upper_bound: Option<u64>,
    },
}

impl BucketResult {
    pub(crate) fn empty_from_req(req: &BucketAggregationInternal) -> crate::Result<Self> {
        let empty_bucket = IntermediateBucketResult::empty_from_req(&req.bucket_agg);
        BucketResult::from_intermediate_and_req(empty_bucket, req)
    }

    fn from_intermediate_and_req(
        bucket_result: IntermediateBucketResult,
        req: &BucketAggregationInternal,
    ) -> crate::Result<Self> {
        match bucket_result {
            IntermediateBucketResult::Range(range_res) => {
                let mut buckets: Vec<RangeBucketEntry> = range_res
                    .buckets
                    .into_iter()
                    .map(|(_, bucket)| {
                        RangeBucketEntry::from_intermediate_and_req(bucket, &req.sub_aggregation)
                    })
                    .collect::<crate::Result<Vec<_>>>()?;

                buckets.sort_by(|left, right| {
                    // TODO use total_cmp next stable rust release
                    left.from
                        .unwrap_or(f64::MIN)
                        .partial_cmp(&right.from.unwrap_or(f64::MIN))
                        .unwrap_or(Ordering::Equal)
                });
                Ok(BucketResult::Range { buckets })
            }
            IntermediateBucketResult::Histogram { buckets } => {
                let buckets = intermediate_buckets_to_final_buckets(
                    buckets,
                    req.as_histogram()
                        .expect("unexpected aggregation, expected histogram aggregation"),
                    &req.sub_aggregation,
                )?;

                Ok(BucketResult::Histogram { buckets })
            }
            IntermediateBucketResult::Terms(terms) => terms.into_final_result(
                req.as_term()
                    .expect("unexpected aggregation, expected term aggregation"),
                &req.sub_aggregation,
            ),
        }
    }
}

/// This is the default entry for a bucket, which contains a key, count, and optionally
/// sub_aggregations.
///
/// # JSON Format
/// ```json
/// {
///   ...
///     "my_histogram": {
///       "buckets": [
///         {
///           "key": "2.0",
///           "doc_count": 5
///         },
///         {
///           "key": "4.0",
///           "doc_count": 2
///         },
///         {
///           "key": "6.0",
///           "doc_count": 3
///         }
///       ]
///    }
///    ...
/// }
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BucketEntry {
    /// The identifier of the bucket.
    pub key: Key,
    /// Number of documents in the bucket.
    pub doc_count: u64,
    #[serde(flatten)]
    /// Sub-aggregations in this bucket.
    pub sub_aggregation: AggregationResults,
}

impl BucketEntry {
    pub(crate) fn from_intermediate_and_req(
        entry: IntermediateHistogramBucketEntry,
        req: &AggregationsInternal,
    ) -> crate::Result<Self> {
        Ok(BucketEntry {
            key: Key::F64(entry.key),
            doc_count: entry.doc_count,
            sub_aggregation: AggregationResults::from_intermediate_and_req_internal(
                entry.sub_aggregation,
                req,
            )?,
        })
    }
}
impl GetDocCount for &BucketEntry {
    fn doc_count(&self) -> u64 {
        self.doc_count
    }
}
impl GetDocCount for BucketEntry {
    fn doc_count(&self) -> u64 {
        self.doc_count
    }
}

/// This is the range entry for a bucket, which contains a key, count, and optionally
/// sub_aggregations.
///
/// # JSON Format
/// ```json
/// {
///   ...
///     "my_ranges": {
///       "buckets": [
///         {
///           "key": "*-10",
///           "to": 10,
///           "doc_count": 5
///         },
///         {
///           "key": "10-20",
///           "from": 10,
///           "to": 20,
///           "doc_count": 2
///         },
///         {
///           "key": "20-*",
///           "from": 20,
///           "doc_count": 3
///         }
///       ]
///    }
///    ...
/// }
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RangeBucketEntry {
    /// The identifier of the bucket.
    pub key: Key,
    /// Number of documents in the bucket.
    pub doc_count: u64,
    #[serde(flatten)]
    /// sub-aggregations in this bucket.
    pub sub_aggregation: AggregationResults,
    /// The from range of the bucket. Equals f64::MIN when None.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<f64>,
    /// The to range of the bucket. Equals f64::MAX when None.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<f64>,
}

impl RangeBucketEntry {
    fn from_intermediate_and_req(
        entry: IntermediateRangeBucketEntry,
        req: &AggregationsInternal,
    ) -> crate::Result<Self> {
        Ok(RangeBucketEntry {
            key: entry.key,
            doc_count: entry.doc_count,
            sub_aggregation: AggregationResults::from_intermediate_and_req_internal(
                entry.sub_aggregation,
                req,
            )?,
            to: entry.to,
            from: entry.from,
        })
    }
}