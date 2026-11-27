use k8s_openapi::api::core::v1::Pod;
use k8s_openapi::jiff::Timestamp;

/// The apiserver's own wall clock, read from the HTTP `Date` header of a list
/// response.
///
/// Every age comparison against cluster objects uses this rather than the
/// worker host's clock. Worker replicas each stamp their own local time, so a
/// host skewed fast does not merely race with its peers: it deterministically
/// computes every pod they just created as expired, on every sweep. Reading the
/// clock from the same authority that stamped `creationTimestamp` removes the
/// comparison's dependence on host time entirely.
pub type ApiServerClock = Timestamp;

/// Age of `pod` against the apiserver clock, in whole seconds.
///
/// `creationTimestamp` is apiserver-stamped, so both sides of this subtraction
/// come from the same clock. `None` means the pod carried no creation timestamp
/// (which a listed object always does) and therefore has no age this code is
/// willing to guess at.
pub fn pod_age_seconds(pod: &Pod, now: ApiServerClock) -> Option<i64> {
    let created = pod.metadata.creation_timestamp.as_ref()?;
    Some(now.as_second() - created.0.as_second())
}

/// Parse an HTTP `Date` header (RFC 9110 IMF-fixdate, e.g.
/// `Tue, 15 Nov 1994 08:12:31 GMT`).
///
/// The fixed `GMT` zone is rewritten to a numeric offset because the RFC 2822
/// parser wants one; anything else is rejected as unparseable, which callers
/// read as "no clock".
pub(crate) fn parse_http_date(value: &str) -> Option<ApiServerClock> {
    let value = value.trim();
    let normalized = match value.strip_suffix("GMT") {
        Some(prefix) => format!("{prefix}+0000"),
        None => value.to_string(),
    };
    k8s_openapi::jiff::fmt::rfc2822::parse(&normalized)
        .ok()
        .map(|zoned| zoned.timestamp())
}
