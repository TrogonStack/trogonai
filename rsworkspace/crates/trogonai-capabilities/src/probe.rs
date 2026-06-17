use trogonai_session_contracts::CapabilityTestResult;

/// Contract probe kinds recommended by the capability registry freshness policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeKind {
    ToolUse,
    ImageInput,
    JsonSchema,
    ContextLimits,
    Streaming,
    CompactionSupported,
}

impl ProbeKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::ToolUse => "tool_use",
            Self::ImageInput => "image_input",
            Self::JsonSchema => "json_schema",
            Self::ContextLimits => "context_limits",
            Self::Streaming => "streaming",
            Self::CompactionSupported => "compaction_supported",
        }
    }
}

/// Result of a capability probe or contract test.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeResult {
    pub kind: ProbeKind,
    pub passed: bool,
    pub detail: Option<String>,
}

impl ProbeResult {
    pub fn into_test_result(self) -> CapabilityTestResult {
        CapabilityTestResult {
            name: self.kind.name().to_string(),
            passed: self.passed,
            detail: self.detail,
            ..CapabilityTestResult::default()
        }
    }
}

/// Capability probe contract used by runners to certify model capabilities.
pub trait CapabilityProbe {
    fn probe(&self, kind: ProbeKind) -> ProbeResult;
}

/// Static probe backed by known-good capability flags.
#[derive(Debug, Clone)]
pub struct StaticProbe {
    pub tool_use: bool,
    pub image_input: bool,
    pub json_schema: bool,
    pub max_context_tokens: u64,
    pub streaming: bool,
    pub compaction_supported: bool,
}

impl CapabilityProbe for StaticProbe {
    fn probe(&self, kind: ProbeKind) -> ProbeResult {
        let (passed, detail) = match kind {
            ProbeKind::ToolUse => (self.tool_use, None),
            ProbeKind::ImageInput => (self.image_input, None),
            ProbeKind::JsonSchema => (self.json_schema, None),
            ProbeKind::ContextLimits => (
                self.max_context_tokens >= 8_192,
                Some(format!("max_context_tokens={}", self.max_context_tokens)),
            ),
            ProbeKind::Streaming => (self.streaming, None),
            ProbeKind::CompactionSupported => (self.compaction_supported, None),
        };
        ProbeResult {
            kind,
            passed,
            detail,
        }
    }
}

/// The standard contract-test battery (§ Capability Registry Freshness).
pub const PROBE_BATTERY: [ProbeKind; 6] = [
    ProbeKind::ToolUse,
    ProbeKind::ImageInput,
    ProbeKind::JsonSchema,
    ProbeKind::ContextLimits,
    ProbeKind::Streaming,
    ProbeKind::CompactionSupported,
];

/// Run the standard probe battery and collect contract test results.
pub fn run_probe_battery(probe: &impl CapabilityProbe) -> Vec<ProbeResult> {
    PROBE_BATTERY.into_iter().map(|kind| probe.probe(kind)).collect()
}

/// Async transport that runs a single capability contract test against a **live**
/// runner/model and reports whether the capability actually worked. The runner host
/// implements it (e.g. over the ACP bridge); the capabilities crate stays transport
/// agnostic. This is the runner-backed probe the registry-freshness policy asks for
/// ("health checks/probes por runner"), distinct from the static [`StaticProbe`].
pub trait CapabilityProbeTransport {
    fn run_probe(&self, kind: ProbeKind) -> impl std::future::Future<Output = ProbeResult> + Send;
}

/// Runner-backed probe: issues the full contract-test battery against a live runner via
/// a [`CapabilityProbeTransport`], producing results that
/// [`crate::certification::ProviderCertificationMatrix::certify_from_probes`] turns into
/// a verified certification level.
pub struct RunnerCapabilityProbe<T> {
    transport: T,
}

impl<T: CapabilityProbeTransport> RunnerCapabilityProbe<T> {
    pub fn new(transport: T) -> Self {
        Self { transport }
    }

    /// Run every probe in [`PROBE_BATTERY`] against the live runner, in order.
    pub async fn run_battery(&self) -> Vec<ProbeResult> {
        let mut results = Vec::with_capacity(PROBE_BATTERY.len());
        for kind in PROBE_BATTERY {
            results.push(self.transport.run_probe(kind).await);
        }
        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockTransport {
        fail: Option<ProbeKind>,
    }

    impl CapabilityProbeTransport for MockTransport {
        async fn run_probe(&self, kind: ProbeKind) -> ProbeResult {
            ProbeResult {
                kind,
                passed: self.fail != Some(kind),
                detail: None,
            }
        }
    }

    #[tokio::test]
    async fn runner_probe_runs_full_battery() {
        let probe = RunnerCapabilityProbe::new(MockTransport { fail: None });
        let results = probe.run_battery().await;
        assert_eq!(results.len(), PROBE_BATTERY.len());
        assert!(results.iter().all(|r| r.passed));
    }

    #[tokio::test]
    async fn runner_probe_reports_failed_capability() {
        let probe = RunnerCapabilityProbe::new(MockTransport {
            fail: Some(ProbeKind::ImageInput),
        });
        let results = probe.run_battery().await;
        let image = results.iter().find(|r| r.kind == ProbeKind::ImageInput).unwrap();
        assert!(!image.passed);
    }
}
