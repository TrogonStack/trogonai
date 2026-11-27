use crate::constants::{LABELS, PULL_BYTES, PULL_DURATION, PUSH_BYTES, PUSH_DURATION, REFERRERS_FALLBACK};
use crate::metric::Metric;
use prometheus::{Encoder, HistogramOpts, HistogramVec, IntCounterVec, Opts, Registry, TextEncoder};

/// Prometheus instruments for registry transfers, held in a registry of their own.
#[derive(Clone)]
pub struct OciMetrics {
    registry: Registry,
    pub push_bytes_total: IntCounterVec,
    pub pull_bytes_total: IntCounterVec,
    pub push_duration_seconds: HistogramVec,
    pub pull_duration_seconds: HistogramVec,
    pub referrers_fallback_total: IntCounterVec,
}

impl OciMetrics {
    pub fn new() -> Result<Self, prometheus::Error> {
        let registry = Registry::new();
        let push_bytes_total = counter(&registry, &PUSH_BYTES, &LABELS)?;
        let pull_bytes_total = counter(&registry, &PULL_BYTES, &LABELS)?;
        let push_duration_seconds = histogram(&registry, &PUSH_DURATION)?;
        let pull_duration_seconds = histogram(&registry, &PULL_DURATION)?;
        let referrers_fallback_total = counter(&registry, &REFERRERS_FALLBACK, &["registry"])?;
        Ok(Self {
            registry,
            push_bytes_total,
            pull_bytes_total,
            push_duration_seconds,
            pull_duration_seconds,
            referrers_fallback_total,
        })
    }

    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// The registered metrics in the Prometheus text exposition format.
    pub fn gather_text(&self) -> Result<String, prometheus::Error> {
        let mut buffer = Vec::new();
        TextEncoder::new().encode(&self.registry.gather(), &mut buffer)?;
        Ok(String::from_utf8_lossy(&buffer).into_owned())
    }
}

fn counter(registry: &Registry, metric: &Metric, labels: &[&str]) -> Result<IntCounterVec, prometheus::Error> {
    let counter = IntCounterVec::new(Opts::new(metric.name, metric.help), labels)?;
    registry.register(Box::new(counter.clone()))?;
    Ok(counter)
}

fn histogram(registry: &Registry, metric: &Metric) -> Result<HistogramVec, prometheus::Error> {
    let buckets = prometheus::exponential_buckets(0.05, 2.0, 12)?;
    let options = HistogramOpts::new(metric.name, metric.help).buckets(buckets);
    let histogram = HistogramVec::new(options, &LABELS)?;
    registry.register(Box::new(histogram.clone()))?;
    Ok(histogram)
}
