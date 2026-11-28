//! What the web tools report while they work.

use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::tokens::ToolUsage;

/// What a tool sent out to the network.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolNetworkCallKind {
    /// A query to a search backend.
    WebSearch,
    /// A request for one page.
    WebFetch,
}

/// One request a tool sent out to the network, as the tool timed it.
///
/// A tool is registered once per process and may be served from a component,
/// so it cannot write into the session's call journal. It reports each request
/// on its way out, failed ones included, and whoever runs the session places
/// it in the turn and writes it down.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolNetworkCall {
    /// What went out.
    pub kind: ToolNetworkCallKind,
    /// When the request went out.
    pub started_at: SystemTime,
    /// How long until it settled.
    pub duration: Duration,
    /// The HTTP status it was answered with, when it was answered.
    #[serde(default)]
    pub status: Option<u16>,
    /// How many bytes came back.
    #[serde(default)]
    pub response_bytes: u64,
    /// The model that answered it, when a model did.
    #[serde(default)]
    pub model: Option<String>,
    /// What that model spent.
    #[serde(default)]
    pub usage: Option<ToolUsage>,
    /// Why the answer stopped, when the backend said.
    #[serde(default)]
    pub stop_reason: Option<String>,
    /// The name of the way it failed, when it did.
    #[serde(default)]
    pub failure: Option<String>,
}

impl ToolNetworkCall {
    /// A request of `kind` that went out at `started_at` and settled after
    /// `duration`, with nothing else known about it yet.
    #[must_use]
    pub const fn new(kind: ToolNetworkCallKind, started_at: SystemTime, duration: Duration) -> Self {
        Self {
            kind,
            started_at,
            duration,
            status: None,
            response_bytes: 0,
            model: None,
            usage: None,
            stop_reason: None,
            failure: None,
        }
    }
}

/// How far a web search has got.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebSearchProgress {
    /// A query went out.
    QueryStarted(String),
    /// A query came back.
    ResultsReceived {
        /// The query that came back.
        query: String,
        /// How many results it found.
        result_count: usize,
    },
}

/// What a finished web search cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct WebSearchCompletion {
    /// How many queries went out.
    pub searches: u32,
    /// How long they took together.
    pub duration: Duration,
}

/// How far a web fetch has got.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebFetchProgress {
    /// Asking for the page.
    Fetching(String),
    /// Turning what came back into something a model can read.
    Converting(String),
}

/// Whether the fetched page was kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebFetchArtifactState {
    /// Nothing was kept.
    #[default]
    None,
    /// The page is in the artifact store.
    Stored,
    /// The page was meant to be kept and is not there.
    Unavailable,
}

/// A URL, shortened to what a status line can hold.
///
/// Mirrors the 192-byte inline buffer on `WebFetchCompletion`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub struct FetchedUrl(String);

impl FetchedUrl {
    /// The most a recorded URL can hold.
    pub const MAX_BYTES: usize = 192;

    /// Records a URL, shortening it on a character boundary if it is long.
    #[must_use]
    pub fn new(url: &str) -> Self {
        if url.len() <= Self::MAX_BYTES {
            return Self(url.to_owned());
        }

        let mut kept = Self::MAX_BYTES;
        while !url.is_char_boundary(kept) {
            kept -= 1;
        }
        Self(url[..kept].to_owned())
    }

    /// The URL, at most [`Self::MAX_BYTES`] of it.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether any URL was recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<String> for FetchedUrl {
    fn from(url: String) -> Self {
        Self::new(&url)
    }
}

impl From<FetchedUrl> for String {
    fn from(url: FetchedUrl) -> Self {
        url.0
    }
}

impl std::fmt::Display for FetchedUrl {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// What a finished web fetch got.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct WebFetchCompletion {
    /// What was fetched.
    pub url: FetchedUrl,
    /// How much came back.
    pub bytes: u64,
    /// What the server answered with, when it answered at all.
    pub status: Option<u16>,
    /// How long it took.
    pub duration: Duration,
    /// Whether the answer came from the runtime's own cache rather than the network.
    pub cache_hit: bool,
    /// Whether the page was kept.
    pub artifact_state: WebFetchArtifactState,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_url_that_fits_is_recorded_whole() {
        let url = "https://example.com/a/path?query=1";
        assert_eq!(FetchedUrl::new(url).as_str(), url);
        assert!(FetchedUrl::default().is_empty());
    }

    #[test]
    fn a_long_url_is_cut_on_a_character_boundary() {
        let long = format!("https://example.com/{}", "é".repeat(200));
        let recorded = FetchedUrl::new(&long);

        assert!(recorded.as_str().len() <= FetchedUrl::MAX_BYTES);
        assert!(long.starts_with(recorded.as_str()));
        assert!(recorded.as_str().ends_with('é'));
    }

    #[test]
    fn a_url_exactly_at_the_bound_is_not_shortened() {
        let exact = "x".repeat(FetchedUrl::MAX_BYTES);
        assert_eq!(FetchedUrl::new(&exact).as_str(), exact);

        let over = "x".repeat(FetchedUrl::MAX_BYTES + 1);
        assert_eq!(FetchedUrl::new(&over).as_str().len(), FetchedUrl::MAX_BYTES);
    }

    #[test]
    fn a_fetch_that_never_reached_a_server_reports_no_status() {
        let completion = WebFetchCompletion::default();
        assert_eq!(completion.status, None);
        assert_eq!(completion.duration, Duration::ZERO);
        assert_eq!(completion.artifact_state, WebFetchArtifactState::None);
    }

    #[test]
    fn a_search_reports_what_each_query_found() {
        let progress = WebSearchProgress::ResultsReceived {
            query: "rust wasm".to_owned(),
            result_count: 8,
        };
        let encoded = serde_json::to_string(&progress).expect("progress encodes");
        assert!(encoded.contains("\"results_received\""));
        assert_eq!(
            serde_json::from_str::<WebSearchProgress>(&encoded).expect("progress decodes"),
            progress,
        );
    }

    #[test]
    fn a_fetch_completion_survives_a_round_trip_through_json() {
        let completion = WebFetchCompletion {
            url: FetchedUrl::new("https://example.com/page"),
            bytes: 4_096,
            status: Some(200),
            duration: Duration::from_millis(320),
            cache_hit: true,
            artifact_state: WebFetchArtifactState::Stored,
        };

        let encoded = serde_json::to_string(&completion).expect("a completion encodes");
        assert_eq!(
            serde_json::from_str::<WebFetchCompletion>(&encoded).expect("a completion decodes"),
            completion,
        );
    }
}
