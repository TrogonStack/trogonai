//! The patch queue (`XA`) behind the worker state reports: the patches for `PUT /worker` that wait their turn, merge
//! while they wait, are retried with a backoff until the service takes them, and are put back in pieces when the
//! service refuses part of them.
//!
//! The queue does no I/O. [`PatchQueue::poll`] says what to do next (send a patch, wait, or nothing) and
//! [`PatchQueue::sent`] takes the answer, which is the order the upstream `drain` and `sendWithRetry` loops run in.
//! The upstream options that need a flush policy (`waitsForStatus`, `mergesSameRun`, `remembers`) are fixed off: the
//! bridge entrypoint never sets the policy (bridge.mjs offset 1030319 onward), so they never take effect there.

use std::time::Duration;

use serde_json::{Map, Value};

/// One `PUT /worker` body, without the `worker_epoch` the request adds.
pub type Patch = Map<String, Value>;

const BASE_DELAY_MS: f64 = 500.0;
const MAX_DELAY_MS: f64 = 30_000.0;
const JITTER_MS: f64 = 500.0;
/// The `internal_metadata` keys that only mean something next to a status.
const RIDES_A_STATUS: [&str; 2] = ["reply_degraded", "card_turn_stamp"];
/// The riders a `null` outlives the status for.
const DELETE_OUTLIVES_STATUS: [&str; 1] = ["card_turn_stamp"];
/// The keys that are re-sent one group at a time after a refusal.
const REQUEUED_ALONE: [&str; 1] = ["disabled_mcp_servers"];

fn has_status(patch: &Patch) -> bool {
    patch.contains_key("worker_status")
}

/// `yu`: `later` over `earlier`, with the two metadata maps merged one level deep.
pub fn merge_patches(earlier: &Patch, later: &Patch) -> Patch {
    let mut merged = earlier.clone();
    for (key, value) in later {
        let both = matches!((merged.get(key), value), (Some(Value::Object(_)), Value::Object(_)))
            && (key == "external_metadata" || key == "internal_metadata");
        if both {
            if let Some(Value::Object(held)) = merged.get_mut(key)
                && let Value::Object(incoming) = value
            {
                for (name, item) in incoming {
                    held.insert(name.clone(), item.clone());
                }
            }
        } else {
            merged.insert(key.clone(), value.clone());
        }
    }
    merged
}

/// `um`: what is left of `kept` after the keys `taken` carries are removed from each of its groups, or nothing.
pub fn subtract_patches(kept: &Patch, taken: &Patch) -> Option<Patch> {
    let mut left = Patch::new();
    for (group, value) in kept {
        let Value::Object(items) = value else {
            continue;
        };
        let gone = taken.get(group).and_then(Value::as_object);
        let rest: Map<String, Value> = items
            .iter()
            .filter(|(name, _)| !gone.is_some_and(|gone| gone.contains_key(*name)))
            .map(|(name, item)| (name.clone(), item.clone()))
            .collect();
        if !rest.is_empty() {
            left.insert(group.clone(), Value::Object(rest));
        }
    }
    (!left.is_empty()).then_some(left)
}

/// `withoutStatus`: the part of a patch that is still worth sending once its status is dropped.
pub fn without_status(patch: &Patch) -> Option<Patch> {
    if !has_status(patch) {
        return Some(patch.clone());
    }
    let mut kept = Map::new();
    if let Some(Value::Object(internal)) = patch.get("internal_metadata") {
        for (name, value) in internal {
            let rides = RIDES_A_STATUS.contains(&name.as_str())
                && (!value.is_null() || !DELETE_OUTLIVES_STATUS.contains(&name.as_str()));
            if !rides {
                kept.insert(name.clone(), value.clone());
            }
        }
    }
    if kept.is_empty() {
        return None;
    }
    let mut rest = Patch::new();
    rest.insert("internal_metadata".to_owned(), Value::Object(kept));
    Some(rest)
}

/// What the queue wants done next.
#[derive(Clone, Debug, PartialEq)]
pub enum Next {
    /// Send this patch and report the answer with [`PatchQueue::sent`].
    Send(Patch),
    /// Wait this long, then poll again.
    Pause(Duration),
    /// Nothing is waiting.
    Idle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Send,
    Failed,
    Paused,
}

#[derive(Clone, Debug)]
struct Claim {
    patch: Patch,
    from_requeue: bool,
    attempts: u32,
    rearmed: bool,
    started: bool,
    phase: Phase,
}

/// The queue of `PUT /worker` patches.
#[derive(Debug, Default)]
pub struct PatchQueue {
    claim: Option<Claim>,
    pending: Option<Patch>,
    requeued: Option<Patch>,
    requeued_metadata: Option<Patch>,
    sending: Option<Patch>,
    closed_to_new_patches: bool,
    has_dropped_something: bool,
    cut_kept_metadata: bool,
    closed: bool,
}

impl PatchQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queues a patch. It is merged into the one already waiting; two status patches never merge while the first
    /// can still go out on its own.
    pub fn enqueue(&mut self, patch: Patch) {
        if self.closed || self.closed_to_new_patches {
            self.has_dropped_something = true;
            return;
        }
        if self.pending.as_ref().is_some_and(has_status) && has_status(&patch) {
            self.drain();
        }
        let status = has_status(&patch);
        self.pending = Some(match self.pending.take() {
            Some(waiting) => merge_patches(&waiting, &patch),
            None => patch.clone(),
        });
        if status {
            self.requeued = None;
        }
        if let Some(owed) = self.requeued_metadata.take() {
            self.requeued_metadata = subtract_patches(&owed, &patch);
        }
        self.drain();
    }

    /// Puts a status back after the service refused it together with other fields; a status already waiting wins.
    pub fn requeue_status(&mut self, patch: Patch) {
        if self.closed || self.closed_to_new_patches {
            self.has_dropped_something = true;
            return;
        }
        if self.pending.as_ref().is_some_and(has_status) {
            return;
        }
        self.requeued = Some(patch);
        self.drain();
    }

    /// Puts metadata back after the service refused it together with other fields.
    pub fn requeue_metadata(&mut self, patch: Patch) {
        if self.closed || self.closed_to_new_patches {
            self.has_dropped_something = true;
            return;
        }
        self.put_back_metadata(&patch);
    }

    fn put_back_metadata(&mut self, patch: &Patch) {
        let owed = merge_patches(&self.requeued_metadata.take().unwrap_or_default(), patch);
        self.requeued_metadata = subtract_patches(&owed, &self.pending.clone().unwrap_or_default());
        self.drain();
    }

    /// Stops taking new patches and drops the statuses still waiting; metadata that is owed stays queued.
    pub fn close_to_new_patches(&mut self) {
        if self.closed {
            return;
        }
        self.drain();
        self.closed_to_new_patches = true;
        self.has_dropped_something |= self.requeued.is_some();
        self.requeued = None;
        if let Some(waiting) = self.pending.take() {
            self.has_dropped_something |= has_status(&waiting);
            self.pending = without_status(&waiting);
        }
    }

    pub fn is_closed_to_new_patches(&self) -> bool {
        self.closed_to_new_patches
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// Whether something was dropped or cut that a flush should report as lost.
    pub fn has_dropped_something(&self) -> bool {
        self.has_dropped_something
    }

    /// Whether a status that went out together with kept metadata was cut at close.
    pub fn has_cut_kept_metadata(&self) -> bool {
        self.cut_kept_metadata
    }

    /// Whether the queue, closed to new patches, still holds metadata that has to be sent.
    pub fn owes_metadata(&self) -> bool {
        self.closed_to_new_patches
            && !self.closed
            && (self.pending.is_some()
                || self.requeued_metadata.is_some()
                || self
                    .sending
                    .as_ref()
                    .is_some_and(|sending| without_status(sending).is_some()))
    }

    /// Whether the patch has metadata worth sending without its status.
    pub fn keeps_without_status(&self, patch: &Patch) -> bool {
        without_status(patch).is_some()
    }

    /// Whether a patch is sending or waiting.
    pub fn unsent(&self) -> bool {
        self.claim.is_some() || self.pending.is_some()
    }

    /// Whether anything at all is queued, sending or waiting to be put back.
    pub fn has_work(&self) -> bool {
        self.claim.is_some() || self.pending.is_some() || self.requeued.is_some() || self.requeued_metadata.is_some()
    }

    /// Closes the queue: what has not gone out is dropped. A patch already claimed still makes its first attempt.
    pub fn close(&mut self) {
        self.drain();
        self.closed = true;
        self.pending = None;
        self.requeued = None;
        self.requeued_metadata = None;
    }

    fn drain(&mut self) {
        if self.claim.is_some() || self.closed {
            return;
        }
        let waiting = self.pending.take();
        let from_requeue = waiting.is_none();
        let patch = if waiting.is_some() {
            waiting
        } else if let Some(requeued) = self.requeued.take() {
            Some(requeued)
        } else if let Some(owed) = self.requeued_metadata.clone() {
            let alone: Map<String, Value> = REQUEUED_ALONE
                .iter()
                .map(|key| ((*key).to_owned(), Value::Null))
                .collect();
            let mut marker = Patch::new();
            marker.insert("internal_metadata".to_owned(), Value::Object(alone));
            let others = subtract_patches(&owed, &marker);
            if others.is_none() && self.pending.is_some() {
                return;
            }
            let source = others.as_ref().unwrap_or(&owed);
            let first = source.iter().next().map(|(group, value)| {
                let mut single = Patch::new();
                single.insert(group.clone(), value.clone());
                single
            });
            self.requeued_metadata = first.as_ref().and_then(|single| subtract_patches(&owed, single));
            first
        } else {
            None
        };
        if let Some(patch) = patch {
            self.claim = Some(Claim {
                patch,
                from_requeue,
                attempts: 0,
                rearmed: false,
                started: false,
                phase: Phase::Send,
            });
        }
    }

    /// What to do next. `jitter` is a fresh value in `[0, 1)` for the backoff.
    pub fn poll(&mut self, jitter: f64) -> Next {
        loop {
            if self.claim.is_none() {
                self.drain();
                if self.claim.is_none() {
                    return Next::Idle;
                }
            }
            let Some(claim) = self.claim.clone() else {
                return Next::Idle;
            };
            match claim.phase {
                Phase::Send => {
                    if self.closed && claim.started {
                        self.finish();
                        continue;
                    }
                    if let Some(held) = self.claim.as_mut() {
                        held.started = true;
                    }
                    self.sending = Some(claim.patch.clone());
                    return Next::Send(claim.patch);
                }
                Phase::Failed => {
                    if self.closed || self.hands_back_without_status(&claim.patch) {
                        self.finish();
                        continue;
                    }
                    if self.closed_to_new_patches
                        && claim.from_requeue
                        && !has_status(&claim.patch)
                        && self.pending.is_some()
                        && !self.closed
                    {
                        self.put_back_metadata(&claim.patch);
                        self.finish();
                        continue;
                    }
                    let mut attempts = claim.attempts;
                    let mut rearmed = claim.rearmed;
                    if self.closed_to_new_patches && !rearmed {
                        rearmed = true;
                        attempts = 0;
                    }
                    attempts += 1;
                    if let Some(held) = self.claim.as_mut() {
                        held.attempts = attempts;
                        held.rearmed = rearmed;
                        held.phase = Phase::Paused;
                    }
                    return Next::Pause(retry_delay(attempts, jitter));
                }
                Phase::Paused => {
                    if self.closed || self.hands_back_without_status(&claim.patch) {
                        self.finish();
                        continue;
                    }
                    if let Some(waiting) = self.pending.clone().filter(|_| !self.closed) {
                        if claim.from_requeue {
                            if !has_status(&claim.patch) {
                                self.put_back_metadata(&claim.patch);
                                self.finish();
                                continue;
                            }
                            if has_status(&waiting) {
                                self.finish();
                                continue;
                            }
                        } else if let Some(held) = self.claim.as_mut() {
                            held.patch = merge_patches(&claim.patch, &waiting);
                            self.pending = None;
                        }
                    }
                    if let Some(held) = self.claim.as_mut() {
                        held.phase = Phase::Send;
                    }
                }
            }
        }
    }

    /// Takes the answer to the patch [`Next::Send`] handed out: `true` when the service took it.
    pub fn sent(&mut self, accepted: bool) {
        if accepted {
            self.finish();
        } else if let Some(held) = self.claim.as_mut() {
            held.phase = Phase::Failed;
        }
    }

    fn finish(&mut self) {
        self.claim = None;
        self.sending = None;
    }

    fn hands_back_without_status(&mut self, patch: &Patch) -> bool {
        if !self.closed_to_new_patches || !has_status(patch) {
            return false;
        }
        if self.closed {
            return true;
        }
        self.has_dropped_something = true;
        self.cut_kept_metadata |= self
            .pending
            .as_ref()
            .is_some_and(|waiting| waiting.contains_key("external_metadata"));
        let whole = match &self.pending {
            Some(waiting) => merge_patches(patch, waiting),
            None => patch.clone(),
        };
        self.pending = without_status(&whole);
        true
    }
}

/// The wait before retry number `attempt` (counting from one).
pub fn retry_delay(attempt: u32, jitter: f64) -> Duration {
    let grown =
        (BASE_DELAY_MS * 2_f64.powi(i32::try_from(attempt.saturating_sub(1)).unwrap_or(i32::MAX))).min(MAX_DELAY_MS);
    Duration::from_secs_f64((grown + jitter * JITTER_MS) / 1000.0)
}

const COST_KEYS: [&str; 8] = [
    "cumulative_cost_usd",
    "cumulative_input_tokens",
    "cumulative_output_tokens",
    "cumulative_cache_read_tokens",
    "cumulative_cache_write_tokens",
    "cost_state",
    "memory_extract_place",
    "model_turn_stored",
];
const OWED_KEYS: [&str; 5] = [
    "last_served_model",
    "worker_permission_mode",
    "plan_doc",
    "card_turn_stamp",
    "card_answer_handed_on",
];

/// What goes back into the queue after the service refused a patch with `400`, `413` or `422`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Requeue {
    /// Metadata worth sending again on its own (`requeueMetadata`).
    pub metadata: Option<Patch>,
    /// The status with only what must travel with it (`requeueStatus`).
    pub status: Option<Patch>,
}

fn owed(group: Option<&Value>) -> Map<String, Value> {
    group
        .and_then(Value::as_object)
        .map(|items| {
            items
                .iter()
                .filter(|(name, _)| COST_KEYS.contains(&name.as_str()) || OWED_KEYS.contains(&name.as_str()))
                .map(|(name, item)| (name.clone(), item.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// Works out what a refused patch gives back: the metadata that has to reach the service and the status that was
/// refused only because of what travelled with it.
pub fn requeue_after_refusal(sent: &Patch) -> Requeue {
    let internal = sent
        .get("internal_metadata")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let external = sent
        .get("external_metadata")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let others = sent
        .keys()
        .filter(|key| *key != "internal_metadata" && *key != "external_metadata")
        .count();
    let kept_internal = owed(sent.get("internal_metadata"));
    let kept_external = owed(sent.get("external_metadata"));
    let kept = kept_internal.len() + kept_external.len();
    let mut result = Requeue::default();
    if (!kept_internal.is_empty() && !kept_external.is_empty())
        || (kept > 0 && kept < others + internal.len() + external.len())
    {
        let mut metadata = Patch::new();
        if !kept_internal.is_empty() {
            metadata.insert("internal_metadata".to_owned(), Value::Object(kept_internal));
        }
        if !kept_external.is_empty() {
            metadata.insert("external_metadata".to_owned(), Value::Object(kept_external));
        }
        result.metadata = Some(metadata);
    }
    if let Some(status) = sent.get("worker_status") {
        let mut only = Patch::new();
        only.insert("worker_status".to_owned(), status.clone());
        if let Some(details) = sent.get("requires_action_details") {
            only.insert("requires_action_details".to_owned(), details.clone());
        }
        if let Some(degraded) = internal.get("reply_degraded") {
            let mut rider = Map::new();
            rider.insert("reply_degraded".to_owned(), degraded.clone());
            only.insert("internal_metadata".to_owned(), Value::Object(rider));
        }
        if sent.len() > only.len() || internal.len() > 1 {
            result.status = Some(only);
        }
    }
    result
}
