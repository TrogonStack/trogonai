use crate::enforcement_mode::EnforcementMode;

/// Dry run is a deliberate, visible departure from enforcement, so it is
/// stated again as a warning beside the ordinary startup line.
pub(crate) fn enforcement_notice(mode: EnforcementMode) {
    if mode == EnforcementMode::DryRun {
        // Bound first: line coverage cannot attribute a call inside the macro.
        let mode = mode.as_str();
        tracing::warn!(
            event = "enforcement_disabled",
            mode,
            "dry run: policy denials are not enforced, a caller with no permit is forwarded and audited as would_deny"
        );
    }
}

#[cfg(test)]
mod tests;
