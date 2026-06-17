use std::sync::OnceLock;

use opentelemetry::metrics::{Counter, Histogram};
use opentelemetry::{KeyValue, global};
use trogonai_session_contracts::SwitchResult;

static SWITCH_COMPLETED: OnceLock<Counter<u64>> = OnceLock::new();
static SWITCH_BLOCKED: OnceLock<Counter<u64>> = OnceLock::new();
static SWITCH_CONFIRMATION: OnceLock<Counter<u64>> = OnceLock::new();
static SWITCH_WARNING: OnceLock<Counter<u64>> = OnceLock::new();
static CHECKPOINT_RESULT: OnceLock<Counter<u64>> = OnceLock::new();
static CHECKPOINT_MISMATCH: OnceLock<Counter<u64>> = OnceLock::new();
static CONTEXT_REPAIR: OnceLock<Counter<u64>> = OnceLock::new();
static ARTIFACT_MISSING: OnceLock<Counter<u64>> = OnceLock::new();
static CAPABILITY_DEGRADATION: OnceLock<Counter<u64>> = OnceLock::new();
static SWITCH_RESULT: OnceLock<Counter<u64>> = OnceLock::new();
static RUNNER_ATTACH: OnceLock<Counter<u64>> = OnceLock::new();
static RUNNER_DETACH: OnceLock<Counter<u64>> = OnceLock::new();
static RUNNER_ATTACH_FAILURE: OnceLock<Counter<u64>> = OnceLock::new();
static HYDRATION_FALLBACK: OnceLock<Counter<u64>> = OnceLock::new();
static POST_SWITCH_USER_CORRECTION: OnceLock<Counter<u64>> = OnceLock::new();
static POST_SWITCH_TOOL_FAILURE: OnceLock<Counter<u64>> = OnceLock::new();
static SWITCH_LATENCY: OnceLock<Histogram<f64>> = OnceLock::new();

fn meter() -> opentelemetry::metrics::Meter {
    global::meter("trogonai-switching")
}

fn switch_completed_counter() -> &'static Counter<u64> {
    SWITCH_COMPLETED.get_or_init(|| {
        meter()
            .u64_counter("trogonai.switching.completed")
            .with_description("Model switches completed successfully")
            .build()
    })
}

fn switch_blocked_counter() -> &'static Counter<u64> {
    SWITCH_BLOCKED.get_or_init(|| {
        meter()
            .u64_counter("trogonai.switching.blocked")
            .with_description("Model switches blocked by safety gate")
            .build()
    })
}

fn switch_confirmation_counter() -> &'static Counter<u64> {
    SWITCH_CONFIRMATION.get_or_init(|| {
        meter()
            .u64_counter("trogonai.switching.confirmation_required")
            .with_description("Model switches requiring user confirmation")
            .build()
    })
}

fn switch_warning_counter() -> &'static Counter<u64> {
    SWITCH_WARNING.get_or_init(|| {
        meter()
            .u64_counter("trogonai.switching.allowed_with_warning")
            .with_description("Model switches allowed with explicit warnings")
            .build()
    })
}

fn checkpoint_result_counter() -> &'static Counter<u64> {
    CHECKPOINT_RESULT.get_or_init(|| {
        meter()
            .u64_counter("trogonai.switching.checkpoint_result")
            .with_description("Continuity checkpoint outcomes")
            .build()
    })
}

fn checkpoint_mismatch_counter() -> &'static Counter<u64> {
    CHECKPOINT_MISMATCH.get_or_init(|| {
        meter()
            .u64_counter("trogonai.switching.continuity_mismatch")
            .with_description("Continuity checkpoint mismatches")
            .build()
    })
}

fn context_repair_counter() -> &'static Counter<u64> {
    CONTEXT_REPAIR.get_or_init(|| {
        meter()
            .u64_counter("trogonai.switching.context_repair")
            .with_description("Context repairs after checkpoint mismatch")
            .build()
    })
}

fn artifact_missing_counter() -> &'static Counter<u64> {
    ARTIFACT_MISSING.get_or_init(|| {
        meter()
            .u64_counter("trogonai.switching.artifact_missing")
            .with_description("Missing artifacts during switch")
            .build()
    })
}

fn capability_degradation_counter() -> &'static Counter<u64> {
    CAPABILITY_DEGRADATION.get_or_init(|| {
        meter()
            .u64_counter("trogonai.switching.capability_degradation")
            .with_description("Capability degradations applied during switch")
            .build()
    })
}

fn runner_attach_counter() -> &'static Counter<u64> {
    RUNNER_ATTACH.get_or_init(|| {
        meter()
            .u64_counter("trogonai.switching.runner_attach")
            .with_description("Runner bindings attached")
            .build()
    })
}

fn runner_detach_counter() -> &'static Counter<u64> {
    RUNNER_DETACH.get_or_init(|| {
        meter()
            .u64_counter("trogonai.switching.runner_detach")
            .with_description("Runner bindings detached")
            .build()
    })
}

fn runner_attach_failure_counter() -> &'static Counter<u64> {
    RUNNER_ATTACH_FAILURE.get_or_init(|| {
        meter()
            .u64_counter("trogonai.switching.runner_attach_failure")
            .with_description("Runner attach failures during switch")
            .build()
    })
}

fn hydration_fallback_counter() -> &'static Counter<u64> {
    HYDRATION_FALLBACK.get_or_init(|| {
        meter()
            .u64_counter("trogonai.switching.hydration_fallback")
            .with_description("Canonical hydration fell back to export/import handoff")
            .build()
    })
}

fn post_switch_user_correction_counter() -> &'static Counter<u64> {
    POST_SWITCH_USER_CORRECTION.get_or_init(|| {
        meter()
            .u64_counter("trogonai.switching.post_switch_user_correction")
            .with_description("User corrections issued in the turns immediately after a model switch")
            .build()
    })
}

fn post_switch_tool_failure_counter() -> &'static Counter<u64> {
    POST_SWITCH_TOOL_FAILURE.get_or_init(|| {
        meter()
            .u64_counter("trogonai.switching.post_switch_tool_failure")
            .with_description("Tool failures observed in the turns immediately after a model switch")
            .build()
    })
}

fn switch_latency_histogram() -> &'static Histogram<f64> {
    SWITCH_LATENCY.get_or_init(|| {
        meter()
            .f64_histogram("trogonai.switching.latency_ms")
            .with_description("End-to-end model switch latency in milliseconds")
            .with_unit("ms")
            .build()
    })
}

pub fn record_switch_completed(
    session_id: &str,
    operation_id: &str,
    source_model: &str,
    target_model: &str,
    source_runner: &str,
    target_runner: &str,
) {
    switch_completed_counter().add(
        1,
        &attrs(
            session_id,
            operation_id,
            source_model,
            target_model,
            source_runner,
            target_runner,
            "completed",
            None,
            None,
        ),
    );
}

pub fn record_switch_blocked(session_id: &str, operation_id: &str, target_model: &str, reason_kind: &str) {
    switch_blocked_counter().add(
        1,
        &[
            KeyValue::new("session_id", session_id.to_string()),
            KeyValue::new("operation_id", operation_id.to_string()),
            KeyValue::new("target_model", target_model.to_string()),
            KeyValue::new("reason_kind", reason_kind.to_string()),
            KeyValue::new("switch_result", "blocked"),
        ],
    );
}

pub fn record_switch_confirmation_required(session_id: &str, operation_id: &str, target_model: &str) {
    switch_confirmation_counter().add(
        1,
        &[
            KeyValue::new("session_id", session_id.to_string()),
            KeyValue::new("operation_id", operation_id.to_string()),
            KeyValue::new("target_model", target_model.to_string()),
            KeyValue::new("switch_result", "confirmation_required"),
        ],
    );
}

fn switch_result_counter() -> &'static Counter<u64> {
    SWITCH_RESULT.get_or_init(|| {
        meter()
            .u64_counter("trogonai.switching.result")
            .with_description("Model switches by formal SwitchResult outcome")
            .build()
    })
}

/// Stable, snake_case label for the formal switch outcome (§ Contrato formal de
/// resultado del switch). Every switch normalizes to exactly one of these.
fn switch_result_label(result: SwitchResult) -> &'static str {
    match result {
        SwitchResult::Unspecified => "unspecified",
        SwitchResult::Switched => "switched",
        SwitchResult::Blocked => "blocked",
        SwitchResult::RequiresConfirmation => "requires_confirmation",
        SwitchResult::Degraded => "degraded",
        SwitchResult::Repaired => "repaired",
        SwitchResult::RolledBack => "rolled_back",
        SwitchResult::FailedRecoverable => "failed_recoverable",
        SwitchResult::FailedTerminal => "failed_terminal",
    }
}

/// Record the single, normalized outcome of a switch attempt. The doc requires
/// every result to map to metrics (§ "cada resultado debe mapear a métricas").
pub fn record_switch_result(session_id: &str, operation_id: &str, result: SwitchResult) {
    switch_result_counter().add(
        1,
        &[
            KeyValue::new("session_id", session_id.to_string()),
            KeyValue::new("operation_id", operation_id.to_string()),
            KeyValue::new("switch_result", switch_result_label(result)),
        ],
    );
}

pub fn record_switch_allowed_with_warning(session_id: &str, operation_id: &str, target_model: &str) {
    switch_warning_counter().add(
        1,
        &[
            KeyValue::new("session_id", session_id.to_string()),
            KeyValue::new("operation_id", operation_id.to_string()),
            KeyValue::new("target_model", target_model.to_string()),
            KeyValue::new("switch_result", "allowed_with_warning"),
        ],
    );
}

pub fn record_checkpoint_result(session_id: &str, operation_id: &str, target_model: &str, checkpoint_result: &str) {
    checkpoint_result_counter().add(
        1,
        &[
            KeyValue::new("session_id", session_id.to_string()),
            KeyValue::new("operation_id", operation_id.to_string()),
            KeyValue::new("target_model", target_model.to_string()),
            KeyValue::new("checkpoint_result", checkpoint_result.to_string()),
        ],
    );
}

pub fn record_checkpoint_mismatch(session_id: &str, operation_id: &str, target_model: &str) {
    checkpoint_mismatch_counter().add(
        1,
        &[
            KeyValue::new("session_id", session_id.to_string()),
            KeyValue::new("operation_id", operation_id.to_string()),
            KeyValue::new("target_model", target_model.to_string()),
        ],
    );
}

pub fn record_context_repair(session_id: &str, operation_id: &str, target_model: &str) {
    context_repair_counter().add(
        1,
        &[
            KeyValue::new("session_id", session_id.to_string()),
            KeyValue::new("operation_id", operation_id.to_string()),
            KeyValue::new("target_model", target_model.to_string()),
        ],
    );
}

pub fn record_artifact_missing(session_id: &str, operation_id: &str) {
    artifact_missing_counter().add(
        1,
        &[
            KeyValue::new("session_id", session_id.to_string()),
            KeyValue::new("operation_id", operation_id.to_string()),
        ],
    );
}

pub fn record_capability_degradation(session_id: &str, operation_id: &str, target_model: &str, capability: &str) {
    capability_degradation_counter().add(
        1,
        &[
            KeyValue::new("session_id", session_id.to_string()),
            KeyValue::new("operation_id", operation_id.to_string()),
            KeyValue::new("target_model", target_model.to_string()),
            KeyValue::new("capability_degradation", capability.to_string()),
        ],
    );
}

pub fn record_runner_attached(session_id: &str, runner_id: &str, model_id: &str) {
    runner_attach_counter().add(
        1,
        &[
            KeyValue::new("session_id", session_id.to_string()),
            KeyValue::new("target_runner", runner_id.to_string()),
            KeyValue::new("target_model", model_id.to_string()),
        ],
    );
}

pub fn record_runner_detached(session_id: &str, runner_id: &str, model_id: &str) {
    runner_detach_counter().add(
        1,
        &[
            KeyValue::new("session_id", session_id.to_string()),
            KeyValue::new("source_runner", runner_id.to_string()),
            KeyValue::new("source_model", model_id.to_string()),
        ],
    );
}

pub fn record_runner_attach_failure(session_id: &str, runner_id: &str, model_id: &str) {
    runner_attach_failure_counter().add(
        1,
        &[
            KeyValue::new("session_id", session_id.to_string()),
            KeyValue::new("target_runner", runner_id.to_string()),
            KeyValue::new("target_model", model_id.to_string()),
        ],
    );
}

pub fn record_hydration_fallback(session_id: &str, operation_id: &str, reason: &str) {
    hydration_fallback_counter().add(
        1,
        &[
            KeyValue::new("session_id", session_id.to_string()),
            KeyValue::new("operation_id", operation_id.to_string()),
            KeyValue::new("switch_result", "hydration_fallback"),
            KeyValue::new("reason", redact_telemetry(reason)),
        ],
    );
}

/// Post-switch user-correction signal (drives `post_switch_user_correction_rate`).
pub fn record_post_switch_user_correction(session_id: &str, operation_id: &str, target_model: &str) {
    post_switch_user_correction_counter().add(
        1,
        &[
            KeyValue::new("session_id", session_id.to_string()),
            KeyValue::new("operation_id", operation_id.to_string()),
            KeyValue::new("target_model", target_model.to_string()),
        ],
    );
}

/// Post-switch tool-failure signal (drives `post_switch_tool_failure_rate`).
pub fn record_post_switch_tool_failure(session_id: &str, operation_id: &str, target_model: &str) {
    post_switch_tool_failure_counter().add(
        1,
        &[
            KeyValue::new("session_id", session_id.to_string()),
            KeyValue::new("operation_id", operation_id.to_string()),
            KeyValue::new("target_model", target_model.to_string()),
        ],
    );
}

#[allow(clippy::too_many_arguments)]
pub fn record_switch_latency_ms(
    session_id: &str,
    operation_id: &str,
    latency_ms: f64,
    source_model: &str,
    target_model: &str,
    source_runner: &str,
    target_runner: &str,
    switch_result: &str,
    checkpoint_result: Option<&str>,
) {
    switch_latency_histogram().record(
        latency_ms,
        &attrs(
            session_id,
            operation_id,
            source_model,
            target_model,
            source_runner,
            target_runner,
            switch_result,
            checkpoint_result,
            None,
        ),
    );
}

#[allow(clippy::too_many_arguments)]
fn attrs(
    session_id: &str,
    operation_id: &str,
    source_model: &str,
    target_model: &str,
    source_runner: &str,
    target_runner: &str,
    switch_result: &str,
    checkpoint_result: Option<&str>,
    capability_degradation: Option<&str>,
) -> Vec<KeyValue> {
    let mut values = vec![
        KeyValue::new("session_id", session_id.to_string()),
        KeyValue::new("operation_id", operation_id.to_string()),
        KeyValue::new("source_model", source_model.to_string()),
        KeyValue::new("target_model", target_model.to_string()),
        KeyValue::new("source_runner", source_runner.to_string()),
        KeyValue::new("target_runner", target_runner.to_string()),
        KeyValue::new("switch_result", switch_result.to_string()),
    ];
    if let Some(result) = checkpoint_result {
        values.push(KeyValue::new("checkpoint_result", result.to_string()));
    }
    if let Some(degradation) = capability_degradation {
        values.push(KeyValue::new("capability_degradation", degradation.to_string()));
    }
    values
}

fn redact_telemetry(value: &str) -> String {
    if value.len() > 120 {
        format!("{}…", &value[..119])
    } else {
        value.to_string()
    }
}
