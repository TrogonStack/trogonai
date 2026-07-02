use std::fmt;

/// Classification of an MCP-layer threat. Exactly six variants per
/// MCP-SECURITY-GATEWAY-1.0 section 6.2 / MUST-06.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum McpThreatType {
    ToolPoisoning,
    RugPull,
    CrossServerAttack,
    ConfusedDeputy,
    HiddenInstruction,
    DescriptionInjection,
}

impl McpThreatType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ToolPoisoning => "tool_poisoning",
            Self::RugPull => "rug_pull",
            Self::CrossServerAttack => "cross_server_attack",
            Self::ConfusedDeputy => "confused_deputy",
            Self::HiddenInstruction => "hidden_instruction",
            Self::DescriptionInjection => "description_injection",
        }
    }
}

impl fmt::Display for McpThreatType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_exactly_six_variants_with_stable_names() {
        let names = [
            McpThreatType::ToolPoisoning.as_str(),
            McpThreatType::RugPull.as_str(),
            McpThreatType::CrossServerAttack.as_str(),
            McpThreatType::ConfusedDeputy.as_str(),
            McpThreatType::HiddenInstruction.as_str(),
            McpThreatType::DescriptionInjection.as_str(),
        ];
        assert_eq!(
            names,
            [
                "tool_poisoning",
                "rug_pull",
                "cross_server_attack",
                "confused_deputy",
                "hidden_instruction",
                "description_injection",
            ]
        );
    }
}
