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

/// Run the standard probe battery and collect contract test results.
pub fn run_probe_battery(probe: &impl CapabilityProbe) -> Vec<ProbeResult> {
    [
        ProbeKind::ToolUse,
        ProbeKind::ImageInput,
        ProbeKind::JsonSchema,
        ProbeKind::ContextLimits,
        ProbeKind::Streaming,
        ProbeKind::CompactionSupported,
    ]
    .into_iter()
    .map(|kind| probe.probe(kind))
    .collect()
}
