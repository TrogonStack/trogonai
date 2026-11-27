use prometheus::{Encoder, HistogramOpts, HistogramVec, IntCounterVec, Opts, Registry, TextEncoder};

use crate::VolumeMetricsError;

pub(crate) struct MetricSpec {
    pub(crate) name: &'static str,
    pub(crate) help: &'static str,
    pub(crate) labels: &'static [&'static str],
}

pub(crate) fn gather_text(registry: &Registry) -> Result<String, VolumeMetricsError> {
    let mut buffer = Vec::new();
    TextEncoder::new().encode(&registry.gather(), &mut buffer)?;
    Ok(String::from_utf8(buffer)?)
}

pub(crate) fn counter(registry: &Registry, spec: &MetricSpec) -> Result<IntCounterVec, VolumeMetricsError> {
    let counter = IntCounterVec::new(Opts::new(spec.name, spec.help), spec.labels)?;
    registry.register(Box::new(counter.clone()))?;
    Ok(counter)
}

pub(crate) fn duration_histogram(registry: &Registry, spec: &MetricSpec) -> Result<HistogramVec, VolumeMetricsError> {
    let buckets = prometheus::exponential_buckets(0.01, 2.0, 12)?;
    let opts = HistogramOpts::new(spec.name, spec.help).buckets(buckets);
    let histogram = HistogramVec::new(opts, spec.labels)?;
    registry.register(Box::new(histogram.clone()))?;
    Ok(histogram)
}
