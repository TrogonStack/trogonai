use trogon_std::env::ReadEnv;

pub const ENV_TIER2_MAX_HEADERS_BYTES: &str = "A2A_GATEWAY_TIER2_MAX_HEADERS_BYTES";
pub const ENV_TIER2_MAX_PARAMS_BYTES: &str = "A2A_GATEWAY_TIER2_MAX_PARAMS_BYTES";
pub const ENV_TIER2_MAX_NESTING_DEPTH: &str = "A2A_GATEWAY_TIER2_MAX_NESTING_DEPTH";

// Defaults: 1 MiB snapshot cap and a 64-level nesting cap, applied here
// to Tier-2 evaluation-context headers and request params.
const DEFAULT_MAX_HEADERS_BYTES: usize = 1_048_576;
const DEFAULT_MAX_PARAMS_BYTES: usize = 1_048_576;
const DEFAULT_MAX_NESTING_DEPTH: usize = 64;

/// Resource caps on [`super::Tier2EvaluationContext`] inputs, enforced at
/// ingress construction time so a hostile or broken caller can't DoS the
/// CEL evaluator with oversized headers, an oversized params payload, or
/// pathologically deep JSON nesting.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Tier2ResourceLimits {
    max_headers_bytes: usize,
    max_params_bytes: usize,
    max_nesting_depth: usize,
}

impl Default for Tier2ResourceLimits {
    fn default() -> Self {
        Self {
            max_headers_bytes: DEFAULT_MAX_HEADERS_BYTES,
            max_params_bytes: DEFAULT_MAX_PARAMS_BYTES,
            max_nesting_depth: DEFAULT_MAX_NESTING_DEPTH,
        }
    }
}

impl Tier2ResourceLimits {
    /// Construct explicit limits. A `0` value is accepted rather than
    /// rejected: it's a legitimate (if extreme) operator choice meaning
    /// "never allow any headers/params/nesting for Tier-2 evaluation",
    /// which still fails closed via the same checks as any other limit
    /// rather than needing a distinct "disabled" representation.
    pub fn new(max_headers_bytes: usize, max_params_bytes: usize, max_nesting_depth: usize) -> Self {
        Self {
            max_headers_bytes,
            max_params_bytes,
            max_nesting_depth,
        }
    }

    /// Load limits from environment, falling back to the safe default on
    /// a missing or unparseable value per-knob. Falling back to the
    /// default (rather than e.g. `usize::MAX`, "unlimited") is
    /// deliberate: an operator typo in one of these env vars must not
    /// silently disable the DoS guard the limit exists to provide.
    pub fn from_env<E: ReadEnv>(env: &E) -> Self {
        let defaults = Self::default();
        Self {
            max_headers_bytes: parse_usize_env(env, ENV_TIER2_MAX_HEADERS_BYTES, defaults.max_headers_bytes),
            max_params_bytes: parse_usize_env(env, ENV_TIER2_MAX_PARAMS_BYTES, defaults.max_params_bytes),
            max_nesting_depth: parse_usize_env(env, ENV_TIER2_MAX_NESTING_DEPTH, defaults.max_nesting_depth),
        }
    }

    pub fn max_headers_bytes(&self) -> usize {
        self.max_headers_bytes
    }

    pub fn max_params_bytes(&self) -> usize {
        self.max_params_bytes
    }

    pub fn max_nesting_depth(&self) -> usize {
        self.max_nesting_depth
    }
}

fn parse_usize_env<E: ReadEnv>(env: &E, key: &str, default: usize) -> usize {
    env.var(key)
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .unwrap_or(default)
}

/// Sum of UTF-8 byte lengths of every header key + value. A direct
/// byte-sum (rather than re-serializing headers to JSON) is cheaper and
/// matches how headers actually arrive over the wire.
pub(crate) fn headers_byte_size<'a>(headers: impl IntoIterator<Item = (&'a String, &'a String)>) -> usize {
    headers.into_iter().map(|(k, v)| k.len() + v.len()).sum()
}

/// Serialized JSON byte length of `params`, used for the params-size
/// check against [`Tier2ResourceLimits::max_params_bytes`].
pub(crate) fn params_byte_size(params: &serde_json::Value) -> usize {
    serde_json::to_vec(params)
        .map(|bytes| bytes.len())
        .unwrap_or(usize::MAX)
}

/// Walk `value`'s Array/Object nesting depth, failing the instant depth
/// exceeds `max_depth` rather than fully walking a maliciously deep
/// structure first.
pub(crate) fn json_nesting_depth_exceeds(value: &serde_json::Value, max_depth: usize) -> bool {
    fn walk(value: &serde_json::Value, depth: usize, max_depth: usize) -> bool {
        match value {
            serde_json::Value::Array(items) => {
                let next_depth = depth + 1;
                if next_depth > max_depth {
                    return true;
                }
                items.iter().any(|item| walk(item, next_depth, max_depth))
            }
            serde_json::Value::Object(map) => {
                let next_depth = depth + 1;
                if next_depth > max_depth {
                    return true;
                }
                map.values().any(|item| walk(item, next_depth, max_depth))
            }
            _ => false,
        }
    }
    walk(value, 0, max_depth)
}

#[cfg(test)]
mod tests;
