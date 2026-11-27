//! Times each step of a lease's life against a worker over the real provider subjects: create, first exec, file
//! write, file read, snapshot and delete. The figures are the worker's plus NATS; broker admission is not in them.

use std::time::{Duration, Instant};

use trogon_sandbox_contract::{
    ArtifactType, CheckpointRunningRequest, ReadFileResponse, RunCommandResponse, SnapshotExportError,
    SnapshotExportOutcome, SnapshotPushTarget, WriteFilesResponse,
};
use trogon_sandbox_protocol::subjects::{provider_checkpoint_running, provider_token};

use crate::constants::{
    EVAL, LATENCY_DELETE_POLL, LATENCY_FILE_BYTES, LATENCY_FILE_PATH, LATENCY_PROBE_TIMEOUT, LATENCY_STEPS,
};
use crate::held::Held;
use crate::run_fields::RunFields;
use crate::{ExecModel, LatencyReport, LatencyStep, Target};

/// Nearest-rank percentile of `sorted`, which must be ascending. `p` is in `0..=100`. An empty slice has none.
pub fn percentile(sorted: &[Duration], p: f64) -> Option<Duration> {
    let len = sorted.len();
    let rank = ((p / 100.0) * len as f64).ceil();
    let rank = if rank.is_finite() && rank >= 1.0 {
        rank as usize
    } else {
        1
    };
    sorted.get(rank.min(len).checked_sub(1)?).copied()
}

/// Times `iterations` lease lifetimes against `target`.
///
/// `snapshot` is where a live checkpoint is pushed. Without one the snapshot step is skipped, because a push needs a
/// registry the worker can reach. A lease counts as deleted once the worker stops answering it; one that is still
/// answered after [`Target::timeout`] is a failed delete.
pub async fn measure_latency(
    target: &Target,
    iterations: usize,
    snapshot: Option<&SnapshotPushTarget>,
) -> LatencyReport {
    let mut steps: Vec<LatencyStep> = LATENCY_STEPS.into_iter().map(LatencyStep::new).collect();
    if target.exec == ExecModel::None {
        for step in &mut steps[1..4] {
            step.skip("worker serves no exec or file verbs");
        }
    }
    if snapshot.is_none() {
        steps[4].skip("no snapshot registry given");
    }
    for _ in 0..iterations {
        let started = Instant::now();
        let Ok(held) = Held::acquire(target).await else {
            steps[0].failures += 1;
            continue;
        };
        steps[0].samples.push(started.elapsed());
        lifetime(target, &held, snapshot, &mut steps).await;
        delete(target, held, &mut steps[5]).await;
    }
    LatencyReport {
        provider: provider_token(target.kind).to_owned(),
        steps,
    }
}

fn probe_argv(exec: ExecModel) -> &'static [&'static str] {
    match exec {
        ExecModel::JsEval => &[EVAL, "1"],
        _ => &["true"],
    }
}

/// Times everything between create and delete on one lease.
async fn lifetime(target: &Target, held: &Held<'_>, snapshot: Option<&SnapshotPushTarget>, steps: &mut [LatencyStep]) {
    if target.exec != ExecModel::None {
        let started = Instant::now();
        match held.run(probe_argv(target.exec), false).await {
            Ok(RunCommandResponse::Completed(out)) if out.exit_code == 0 => steps[1].samples.push(started.elapsed()),
            _ => steps[1].failures += 1,
        }
    }
    match target.exec {
        ExecModel::Shell => {
            let started = Instant::now();
            match held.write(LATENCY_FILE_PATH, &[b'x'; LATENCY_FILE_BYTES]).await {
                Ok(WriteFilesResponse::Written) => steps[2].samples.push(started.elapsed()),
                _ => steps[2].failures += 1,
            }
            let started = Instant::now();
            match held.read(LATENCY_FILE_PATH, target.timeout).await {
                Ok(ReadFileResponse::Content { data }) if data.len() == LATENCY_FILE_BYTES => {
                    steps[3].samples.push(started.elapsed());
                }
                _ => steps[3].failures += 1,
            }
        }
        ExecModel::JsEval => {
            steps[2].skip("JS runtime has no file verbs");
            steps[3].skip("JS runtime has no file verbs");
        }
        ExecModel::None => {}
    }
    if let Some(push) = snapshot {
        let request = CheckpointRunningRequest {
            lease_id: held.id.clone(),
            target: push.clone(),
            native_name: None,
        };
        let started = Instant::now();
        let outcome = target
            .call::<SnapshotExportOutcome>(
                provider_checkpoint_running(target.kind, &held.id),
                &request,
                target.timeout,
            )
            .await;
        match outcome {
            Ok(SnapshotExportOutcome::Ok(_)) => steps[4].samples.push(started.elapsed()),
            Ok(SnapshotExportOutcome::Err(SnapshotExportError::Unsupported { message })) => {
                steps[4].skip(format!("unsupported: {message}"));
            }
            _ => steps[4].failures += 1,
        }
    }
}

/// Cancels the lease and times how long until the worker stops serving it.
///
/// Cancel gets no reply, so the only observable end is the worker no longer answering a probe. The figure is the
/// send time of the first probe that went unanswered, accurate to one probe round trip. A worker with no exec verbs
/// cannot be probed, so its delete is not timed.
async fn delete(target: &Target, held: Held<'_>, step: &mut LatencyStep) {
    if target.exec == ExecModel::None {
        step.skip("worker serves no verb to probe a deleted lease with");
        held.release().await;
        return;
    }
    let started = Instant::now();
    target.cancel(&held.id).await;
    while started.elapsed() < target.timeout {
        // A gone lease shows up as a probe that is not answered. Record when that probe was sent, not when it gave
        // up, or every delete would read as the probe timeout.
        let sent = started.elapsed();
        let probe = held
            .run_within(probe_argv(target.exec), RunFields::default(), LATENCY_PROBE_TIMEOUT)
            .await;
        if !matches!(probe, Ok(RunCommandResponse::Completed(_))) {
            step.samples.push(sent);
            return;
        }
        tokio::time::sleep(LATENCY_DELETE_POLL).await;
    }
    step.failures += 1;
}

fn millis(duration: Duration) -> String {
    format!("{:.1}", duration.as_secs_f64() * 1000.0)
}

fn note(step: &LatencyStep) -> String {
    let failed = (step.failures > 0).then(|| format!("{} failed", step.failures));
    [step.skipped.clone(), failed]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("; ")
}

/// Renders the reports as one aligned text table, times in milliseconds.
pub fn render_latency(reports: &[LatencyReport]) -> String {
    let header = ["provider", "step", "n", "p50 ms", "p95 ms", "note"].map(String::from);
    let mut rows = vec![header];
    for report in reports {
        for step in &report.steps {
            let mut sorted = step.samples.clone();
            sorted.sort();
            let cell = |p| percentile(&sorted, p).map_or_else(|| "-".to_owned(), millis);
            rows.push([
                report.provider.clone(),
                step.step.to_owned(),
                sorted.len().to_string(),
                cell(50.0),
                cell(95.0),
                note(step),
            ]);
        }
    }
    let mut widths = [0; 6];
    for row in &rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.len());
        }
    }
    rows.iter()
        .map(|row| {
            let cells: Vec<String> = row
                .iter()
                .zip(widths)
                .enumerate()
                .map(|(column, (cell, width))| match column {
                    // Numbers line up on the right, text on the left, and the last column is not padded.
                    2..=4 => format!("{cell:>width$}"),
                    5 => cell.clone(),
                    _ => format!("{cell:<width$}"),
                })
                .collect();
            cells.join("  ").trim_end().to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The snapshot push target for a registry and repository.
pub fn snapshot_push_target(registry: &str, repository: &str) -> SnapshotPushTarget {
    SnapshotPushTarget {
        registry: registry.to_owned(),
        repository: repository.to_owned(),
        artifact_type: ArtifactType::SandboxRootfsV1,
    }
}

#[cfg(test)]
mod tests;
