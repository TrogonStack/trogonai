//! TypeScript SDK 0.3.323 `forkSession` (`NB`) and the transcript-state loader it reads (`FMe`, `HMe`, `DB`).
//!
//! The Python fork rewrites parent links and session ids line by line. TypeScript instead loads the transcript
//! state (messages, content replacements, data-flow records, derived-from records and more), optionally trims the
//! transcript at `upToMessageId` with the compaction rules (`jMe`, `KMe`, `GMe`), then re-emits every entry under
//! fresh uuids followed by the state records and a custom title.
//!
//! Entries are [`JsonObject`]s so key order matches the JavaScript object spreads of the bundle. The id source and
//! the clock are injected so tests are deterministic.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::{
    JsonObject,
    constants::{
        TS_BASE58_ALPHABET, TS_DERIVED_TYPES, TS_DERIVED_VIA, TS_FLOW_TOUCHED_ACTIONS, TS_FLOW_TOUCHED_KINDS,
        TS_FLOW_VERSION, TS_FORK_CLEARED_KEYS, TS_FORK_MESSAGE_KEYS, TS_FORK_SEGMENT_KEYS, TS_FORK_TITLE_FALLBACK,
        TS_INNER_CALL_LIMIT, TS_MAX_IDENTIFIER_UNITS, TS_SAFE_INTEGER, TS_TRANSCRIPT_KINDS,
    },
    session_messages_ts::{
        MessagesError, command_string, flag, kind_of, link, parent_uuid, queued_command, str_field, uuid_of,
    },
    session_prompt::{iso_from_epoch_ms, js_trim},
};

/// Why the TypeScript fork refuses a session.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ForkTsError {
    #[error("Session {0} has no messages to fork")]
    NoMessages(String),
    #[error("Message {message_id} not found in session {session_id}")]
    MessageNotFound { message_id: String, session_id: String },
    /// The chain rebuild threw on malformed compaction metadata, as the bundle does.
    #[error(transparent)]
    Messages(#[from] MessagesError),
}

/// `{type, id, via}` of a `derived-from` record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Derived {
    kind: String,
    id: Option<String>,
    via: String,
}

impl Derived {
    fn session(id: &str, via: &str) -> Self {
        Self {
            kind: "claude_code_session".to_owned(),
            id: Some(id.to_owned()),
            via: via.to_owned(),
        }
    }

    fn identity(&self) -> String {
        format!("{}\n{}\n{}", self.kind, self.id.as_deref().unwrap_or(""), self.via)
    }

    fn to_value(&self) -> Value {
        Value::Object(object([
            ("type", Value::String(self.kind.clone())),
            ("id", self.id.clone().map_or(Value::Null, Value::String)),
            ("via", Value::String(self.via.clone())),
        ]))
    }
}

/// `Dp`: drop later duplicates by `(type, id, via)`.
fn dedupe_derived(items: Vec<Derived>) -> Vec<Derived> {
    let mut seen = HashSet::new();
    items.into_iter().filter(|item| seen.insert(item.identity())).collect()
}

/// The data-flow section of a transcript state.
#[derive(Debug, Clone, Default)]
pub(crate) struct DataFlow {
    pub(crate) records: Vec<JsonObject>,
    pub(crate) incomplete: bool,
}

/// What `FMe` and `HMe` return.
#[derive(Debug, Clone, Default)]
pub(crate) struct TranscriptState {
    pub(crate) transcript: Vec<JsonObject>,
    pub(crate) replacements: Vec<JsonObject>,
    pub(crate) relocated_cwd: Option<String>,
    pub(crate) history_suppressed: bool,
    pub(crate) atis_latch: Option<String>,
    pub(crate) tools_used: Option<Vec<String>>,
    pub(crate) data_flow: Option<DataFlow>,
    pub(crate) derived_from: Option<Vec<Derived>>,
}

fn object<const N: usize>(pairs: [(&str, Value); N]) -> JsonObject {
    pairs.into_iter().map(|(key, value)| (key.to_owned(), value)).collect()
}

fn string_value(text: &str) -> Value {
    Value::String(text.to_owned())
}

/// `or`: a non-empty string of at most 256 UTF-16 units without control characters.
fn identifier(value: Option<&Value>) -> Option<&str> {
    let text = value?.as_str()?;
    let units = text.encode_utf16().count();
    let clean = !text.chars().any(|ch| matches!(ch, '\u{0}'..='\u{1f}' | '\u{7f}'));
    (units > 0 && units <= TS_MAX_IDENTIFIER_UNITS && clean).then_some(text)
}

fn is_true(value: Option<&Value>) -> bool {
    value == Some(&Value::Bool(true))
}

fn is_version_one(value: Option<&Value>) -> bool {
    value
        .and_then(Value::as_f64)
        .is_some_and(|number| (number - TS_FLOW_VERSION as f64).abs() == 0.0)
}

/// `TMe` and `Sw`: a `..._01<22 base58 characters>` suffix as the uuid it encodes.
fn short_id_as_uuid(id: &str) -> Option<String> {
    let suffix = &id[id.rfind('_')? + 1..];
    let encoded = suffix.strip_prefix("01")?;
    if encoded.chars().count() != 22 {
        return None;
    }
    let mut value: u128 = 0;
    for ch in encoded.chars() {
        let digit = TS_BASE58_ALPHABET.find(ch)?;
        value = value.checked_mul(58)?.checked_add(digit as u128)?;
    }
    let hex = format!("{value:032x}");
    Some(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    ))
}

/// `Ih`: the server a data-flow record names, `None` when it is malformed.
fn known_server(value: &Value) -> Option<JsonObject> {
    let server = value.as_object()?;
    match str_field(server, "type")? {
        "anthropic" => {
            let id = identifier(server.get("id"))?;
            Some(object([("type", string_value("anthropic")), ("id", string_value(id))]))
        }
        "claude_ai_connector" => {
            let id = identifier(server.get("id"))?;
            let directory = match server.get("directory_id") {
                Some(Value::Null) => Value::Null,
                other => string_value(identifier(other)?),
            };
            let canonical = short_id_as_uuid(id).unwrap_or_else(|| id.to_owned());
            Some(object([
                ("type", string_value("claude_ai_connector")),
                ("id", Value::String(canonical)),
                ("directory_id", directory),
            ]))
        }
        "client_configured" => {
            let transport =
                str_field(server, "transport").filter(|name| ["stdio", "http", "in_process"].contains(name))?;
            let hash = match server.get("plugin_id_hash") {
                None => None,
                other => Some(identifier(other)?),
            };
            let mut out = object([
                ("type", string_value("client_configured")),
                ("transport", string_value(transport)),
            ]);
            if let Some(hash) = hash {
                out.insert("plugin_id_hash".to_owned(), string_value(hash));
            }
            Some(out)
        }
        _ => None,
    }
}

/// `IMe`: a validated data-flow record, `None` when it is dropped.
fn flow_record(value: &Value) -> Option<JsonObject> {
    let source = value.as_object()?;
    let mut out = JsonObject::new();
    if let Some(name) = str_field(source, "tool_name") {
        out.insert("tool_name".to_owned(), string_value(name));
    }
    let tool_use = identifier(source.get("tool_use_id"));
    if let Some(id) = tool_use {
        out.insert("tool_use_id".to_owned(), string_value(id));
    }
    let event = identifier(source.get("event_id"));
    if let Some(id) = event {
        out.insert("event_id".to_owned(), string_value(id));
    }
    if tool_use.is_none() && event.is_none() {
        return None;
    }
    if tool_use.is_some() && !out.contains_key("tool_name") {
        out.insert("tool_name".to_owned(), string_value(""));
    }
    if let Some(inner) = source.get("inner_call_index") {
        let number = inner.as_f64()?;
        let safe = number.fract() == 0.0 && number.abs() <= TS_SAFE_INTEGER;
        if !safe || !(0.0..TS_INNER_CALL_LIMIT).contains(&number) {
            return None;
        }
        out.insert("inner_call_index".to_owned(), inner.clone());
    }
    for key in ["no_message", "implicit", "carried"] {
        if is_true(source.get(key)) {
            out.insert(key.to_owned(), Value::Bool(true));
        }
    }
    if let Some(server) = source.get("server") {
        let server = known_server(server).unwrap_or_else(|| object([("type", string_value("unknown"))]));
        out.insert("server".to_owned(), Value::Object(server));
    }
    if let Some(name) = identifier(source.get("mcp_tool_name")) {
        out.insert("mcp_tool_name".to_owned(), string_value(name));
    }
    if let Some(origin) = source.get("source") {
        let origin = origin.as_object()?;
        let kind = identifier(origin.get("type"))?;
        let id = match origin.get("id") {
            Some(Value::Null) => Value::Null,
            other => string_value(identifier(other)?),
        };
        out.insert(
            "source".to_owned(),
            Value::Object(object([("type", string_value(kind)), ("id", id)])),
        );
    }
    if let Some(touched) = source.get("touched") {
        let touched = touched.as_object()?;
        let kind = str_field(touched, "kind").filter(|name| TS_FLOW_TOUCHED_KINDS.contains(name))?;
        let action = str_field(touched, "action").filter(|name| TS_FLOW_TOUCHED_ACTIONS.contains(name))?;
        let id = match touched.get("id") {
            Some(Value::Null) => Value::Null,
            other => string_value(identifier(other)?),
        };
        let mut entry = object([
            ("kind", string_value(kind)),
            ("id", id),
            ("action", string_value(action)),
        ]);
        if let Some(to) = touched.get("to") {
            entry.insert("to".to_owned(), string_value(identifier(Some(to))?));
        }
        out.insert("touched".to_owned(), Value::Object(entry));
    }
    Some(out)
}

/// `pa`: the identity of a record. Two records with the same key describe the same call or event.
fn record_key(record: &JsonObject) -> Option<String> {
    let name = str_field(record, "tool_name").unwrap_or("");
    let inner = record.get("inner_call_index").map(js_stringify);
    let head = if let Some(id) = str_field(record, "tool_use_id") {
        match inner {
            None => format!("call\n{id}\n"),
            Some(inner) => format!("call\n{id}\n{inner}\n{name}"),
        }
    } else if let Some(id) = str_field(record, "event_id") {
        format!("event\n{id}\n{name}\n{}", inner.unwrap_or_default())
    } else {
        return None;
    };
    let origin = record.get("source").and_then(Value::as_object);
    let touched = record.get("touched").and_then(Value::as_object);
    let text = |object: Option<&JsonObject>, key: &str| {
        object
            .and_then(|object| object.get(key))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned()
    };
    Some(
        [
            head,
            text(origin, "type"),
            text(origin, "id"),
            text(touched, "kind"),
            text(touched, "id"),
            text(touched, "action"),
            text(touched, "to"),
        ]
        .join("\n"),
    )
}

/// `xB`: fold one `data-flow-records` or `data-flow-incomplete` entry into the running section.
fn merge_flow(entry: &JsonObject, flow: &mut DataFlow) {
    match str_field(entry, "type") {
        Some("data-flow-incomplete") => {
            flow.incomplete = true;
            return;
        }
        Some("data-flow-records") => {}
        _ => return,
    }
    if !is_version_one(entry.get("v")) {
        flow.incomplete = true;
        return;
    }
    let Some(Value::Array(records)) = entry.get("records") else {
        flow.incomplete = true;
        return;
    };
    let accepted: Vec<JsonObject> = records.iter().filter_map(flow_record).collect();
    if accepted.len() < records.len() {
        flow.incomplete = true;
    }
    if is_true(entry.get("snapshot")) {
        let mut seen = HashSet::new();
        flow.records = accepted
            .into_iter()
            .filter(|record| record_key(record).is_none_or(|key| seen.insert(key)))
            .collect();
        return;
    }
    let mut index: HashMap<String, usize> = HashMap::new();
    for (position, record) in flow.records.iter().enumerate() {
        if let Some(key) = record_key(record) {
            index.insert(key, position);
        }
    }
    for record in accepted {
        let key = record_key(&record);
        let Some(position) = key.as_ref().and_then(|key| index.get(key)).copied() else {
            flow.records.push(record);
            if let Some(key) = key {
                index.insert(key, flow.records.len() - 1);
            }
            continue;
        };
        let existing = &flow.records[position];
        if is_true(existing.get("carried")) && !is_true(record.get("carried")) {
            flow.records[position] = record;
            continue;
        }
        if is_true(record.get("no_message")) && !is_true(existing.get("no_message")) {
            let mut merged = existing.clone();
            merged.insert("no_message".to_owned(), Value::Bool(true));
            flow.records[position] = merged;
        }
    }
}

/// `LMe`: one `derived-from` session, `None` when it is malformed.
fn derived_session(value: &Value) -> Option<Derived> {
    let entry = value.as_object()?;
    let kind = str_field(entry, "type").filter(|name| TS_DERIVED_TYPES.contains(name))?;
    let via = str_field(entry, "via").filter(|name| TS_DERIVED_VIA.contains(name))?;
    let id = if kind == "unknown" {
        matches!(entry.get("id"), Some(Value::Null)).then_some(None)?
    } else {
        Some(identifier(entry.get("id"))?.to_owned())
    };
    Some(Derived {
        kind: kind.to_owned(),
        id,
        via: via.to_owned(),
    })
}

/// `wB`: the sessions of a `derived-from` entry and whether any of them was lost.
fn derived_sessions(entry: &JsonObject) -> (Vec<Derived>, bool) {
    let Some(Value::Array(sessions)) = entry.get("sessions").filter(|_| is_version_one(entry.get("v"))) else {
        return (Vec::new(), true);
    };
    let parsed: Vec<Option<Derived>> = sessions.iter().map(derived_session).collect();
    let lost = parsed.iter().any(Option::is_none);
    (dedupe_derived(parsed.into_iter().flatten().collect()), lost)
}

#[derive(Default)]
struct Scratch {
    state: TranscriptState,
    boundary_uncovered: bool,
}

/// `DB`: fold one entry into the state; the relocated working directory it names, if any.
fn observe(scratch: &mut Scratch, entry: &JsonObject, session_id: &str) -> Option<String> {
    let kind = str_field(entry, "type");
    let own = str_field(entry, "sessionId") == Some(session_id);
    if let Some(kind) = kind
        && TS_TRANSCRIPT_KINDS.contains(&kind)
        && matches!(entry.get("uuid"), Some(Value::String(_)))
    {
        scratch.state.transcript.push(entry.clone());
        if kind == "system"
            && str_field(entry, "subtype") == Some("compact_boundary")
            && entry.get("isSidechain") != Some(&Value::Bool(true))
            && own
        {
            scratch.boundary_uncovered = true;
        }
        return None;
    }
    match kind {
        Some("history-suppression") => scratch.state.history_suppressed = true,
        Some("atis-latch") if own => {
            if let Some(atis) =
                str_field(entry, "atis").filter(|text| text.chars().all(|ch| ('\u{21}'..='\u{7e}').contains(&ch)))
            {
                scratch.state.atis_latch = Some(atis.to_owned());
            }
        }
        Some("tools-used") if own => {
            if let Some(Value::Array(tools)) = entry.get("tools") {
                scratch.state.tools_used = Some(
                    tools
                        .iter()
                        .filter_map(Value::as_str)
                        .filter(|name| !name.is_empty())
                        .map(str::to_owned)
                        .collect(),
                );
            }
        }
        Some("data-flow-records" | "data-flow-incomplete") if own && !entry.contains_key("agentId") => {
            let flow = scratch.state.data_flow.get_or_insert_with(DataFlow::default);
            merge_flow(entry, flow);
            if kind == Some("data-flow-records") && is_true(entry.get("snapshot")) {
                scratch.boundary_uncovered = false;
            }
        }
        Some("derived-from") if own => {
            let (sessions, lost) = derived_sessions(entry);
            if lost {
                let mut all = scratch.state.derived_from.take().unwrap_or_default();
                all.extend(sessions);
                scratch.state.derived_from = Some(dedupe_derived(all));
                scratch.state.data_flow.get_or_insert_with(DataFlow::default).incomplete = true;
            } else {
                scratch.state.derived_from = Some(sessions);
            }
        }
        Some("content-replacement") if own => {
            if let Some(Value::Array(items)) = entry.get("replacements") {
                scratch
                    .state
                    .replacements
                    .extend(items.iter().filter_map(Value::as_object).cloned());
            }
        }
        Some("relocated") if own => {
            return str_field(entry, "relocatedCwd")
                .filter(|cwd| !cwd.is_empty())
                .map(str::to_owned);
        }
        _ => {}
    }
    None
}

/// `PB`: a data-flow section that a later compaction boundary left uncovered is incomplete.
fn finish(mut scratch: Scratch) -> TranscriptState {
    if scratch.boundary_uncovered
        && let Some(flow) = scratch.state.data_flow.as_mut()
    {
        flow.incomplete = true;
    }
    scratch.state
}

/// `FMe`: the state of a transcript read from disk. Blank lines and lines that fail to parse are skipped.
pub(crate) fn load_state(content: &[u8], session_id: &str) -> TranscriptState {
    let mut scratch = Scratch::default();
    let mut relocated = None;
    for line in content.split(|byte| *byte == b'\n') {
        let Some(start) = line.iter().position(|byte| *byte > 32) else {
            continue;
        };
        let text = String::from_utf8_lossy(&line[start..]);
        let Ok(Value::Object(entry)) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        relocated = observe(&mut scratch, &entry, session_id).or(relocated);
    }
    scratch.state.relocated_cwd = relocated;
    finish(scratch)
}

/// `Lh`: the preserved-messages or preserved-segment object of a compaction boundary that has an `anchorUuid`.
fn preserved(entry: &JsonObject) -> Option<&JsonObject> {
    let meta = entry.get("compactMetadata")?.as_object()?;
    let messages = meta.get("preservedMessages").filter(|value| !value.is_null());
    let chosen = messages.or_else(|| meta.get("preservedSegment"))?;
    let chosen = chosen.as_object()?;
    chosen.contains_key("anchorUuid").then_some(chosen)
}

fn anchor_of(entry: &JsonObject) -> Option<&str> {
    preserved(entry)?.get("anchorUuid")?.as_str()
}

/// `UB`: a zero-duration boundary that anchors itself.
fn is_self_anchored(entry: &JsonObject) -> bool {
    let zero = entry
        .get("compactMetadata")
        .and_then(Value::as_object)
        .and_then(|meta| meta.get("durationMs"))
        .and_then(Value::as_f64)
        == Some(0.0);
    zero && anchor_of(entry).is_some_and(|anchor| Some(anchor) == str_field(entry, "uuid"))
}

fn is_boundary(entry: &JsonObject) -> bool {
    kind_of(entry) == Some("system") && str_field(entry, "subtype") == Some("compact_boundary")
}

/// `FB`: a compaction boundary that is not self-anchored.
fn is_compaction(entry: &JsonObject) -> bool {
    is_boundary(entry) && !is_self_anchored(entry)
}

/// `Eh`, restricted to the string elements, the only ones a uuid comparison can match.
fn preserved_uuids(entry: &JsonObject) -> Vec<String> {
    let Some(meta) = entry.get("compactMetadata").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    if let Some(Value::Array(uuids)) = meta
        .get("preservedMessages")
        .and_then(Value::as_object)
        .and_then(|messages| messages.get("uuids"))
    {
        out.extend(uuids.iter().filter_map(Value::as_str).map(str::to_owned));
    }
    if let Some(segment) = meta.get("preservedSegment").and_then(Value::as_object) {
        for key in ["headUuid", "tailUuid"] {
            if let Some(uuid) = segment.get(key).and_then(Value::as_str) {
                out.push(uuid.to_owned());
            }
        }
    }
    out
}

/// `jMe`: drop everything after the last compaction boundary that follows the final turn, keeping the entries the
/// boundary preserved.
fn compact_prefix(entries: &[JsonObject]) -> Vec<JsonObject> {
    let last_turn = entries
        .iter()
        .rposition(|entry| matches!(kind_of(entry), Some("user" | "assistant")));
    let start = last_turn.map_or(0, |index| index + 1);
    let Some(boundary_index) = entries
        .iter()
        .enumerate()
        .skip(start)
        .find(|(_, entry)| is_compaction(entry))
        .map(|(index, _)| index)
    else {
        return entries.to_vec();
    };
    let boundary = &entries[boundary_index];
    let head = &entries[..boundary_index];
    let mut wanted: HashSet<String> = HashSet::new();
    if anchor_of(boundary).is_none_or(|anchor| Some(anchor) != str_field(boundary, "uuid")) {
        wanted.extend(preserved_uuids(boundary));
        if let Some(parent) = str_field(boundary, "logicalParentUuid") {
            wanted.insert(parent.to_owned());
        }
    }
    let matched = head.iter().rposition(|entry| wanted.contains(uuid_of(entry)));
    let last_boundary = head.iter().rfind(|entry| is_boundary(entry));
    let anchored = match (matched, last_boundary) {
        (Some(index), Some(last))
            if !is_self_anchored(last) && preserved_uuids(last).iter().any(|uuid| uuid == uuid_of(&head[index])) =>
        {
            head.iter()
                .rposition(|entry| anchor_of(last).is_some_and(|anchor| anchor == uuid_of(entry)))
        }
        _ => None,
    };
    let kept = match matched.max(anchored) {
        None => head,
        Some(cut) => &head[..=cut],
    };
    if kept.iter().any(|entry| kind_of(entry) != Some("progress")) {
        kept.to_vec()
    } else {
        entries.to_vec()
    }
}

/// `KMe`: the boundary that anchors at the cut message no longer preserves anything.
fn clear_preserved(cut_uuid: &str, entries: Vec<JsonObject>) -> Vec<JsonObject> {
    entries
        .into_iter()
        .map(|mut entry| {
            if is_compaction(&entry)
                && anchor_of(&entry) == Some(cut_uuid)
                && let Some(Value::Object(meta)) = entry.get_mut("compactMetadata")
            {
                for key in ["preservedMessages", "preservedSegment", "postTokens"] {
                    meta.shift_remove(key);
                }
            }
            entry
        })
        .collect()
}

/// `{...entry, ...MB}`: clear the keys a fork does not carry over.
fn clear_fork_keys(entry: &JsonObject) -> JsonObject {
    let mut out = entry.clone();
    out.insert("isSidechain".to_owned(), Value::Bool(false));
    for key in TS_FORK_CLEARED_KEYS {
        out.shift_remove(key);
    }
    out
}

struct Pruning<'a> {
    entries: &'a [JsonObject],
    by_uuid: HashMap<&'a str, &'a JsonObject>,
    live: HashSet<String>,
    index: HashMap<&'a str, usize>,
    start: usize,
    left_out: HashSet<String>,
}

impl Pruning<'_> {
    /// `bw`: the entry is dead and lies at or after the start of the kept chain.
    fn reaches(&self, uuid: &str) -> bool {
        !self.live.contains(uuid) && self.index.get(uuid).copied().unwrap_or(self.start) >= self.start
    }

    /// `kB`: leave out an entry, its dead ancestors and every direct child of the left-out set.
    fn leave_out(&mut self, uuid: &str) {
        let mut current = self.by_uuid.get(uuid).copied();
        while let Some(entry) = current {
            if !self.reaches(uuid_of(entry)) || self.left_out.contains(uuid_of(entry)) {
                break;
            }
            self.left_out.insert(uuid_of(entry).to_owned());
            current = parent_uuid(entry).and_then(|parent| self.by_uuid.get(parent).copied());
        }
        for entry in self.entries {
            if parent_uuid(entry).is_some_and(|parent| self.left_out.contains(parent)) {
                self.left_out.insert(uuid_of(entry).to_owned());
            }
        }
    }
}

/// `GMe`: leave out the dead branches that a truncated slice would otherwise carry into the fork.
fn prune_dead_branches(
    entries: Vec<JsonObject>,
    ids: &mut dyn FnMut() -> String,
) -> Result<Vec<JsonObject>, ForkTsError> {
    let Some(tail) = entries.last() else {
        return Ok(entries);
    };
    let cleared: Vec<JsonObject> = entries.iter().map(clear_fork_keys).collect();
    let mut probe = cleared.clone();
    let mut sentinel = object([("type", string_value("system")), ("uuid", Value::String(ids()))]);
    for (key, from) in [
        ("parentUuid", "uuid"),
        ("sessionId", "sessionId"),
        ("timestamp", "timestamp"),
    ] {
        if let Some(value) = tail.get(from) {
            sentinel.insert(key.to_owned(), value.clone());
        }
    }
    probe.push(sentinel);
    let chain = link(&probe)?.map(|linked| linked.chain).unwrap_or_default();
    let mut index: HashMap<&str, usize> = HashMap::new();
    for (position, entry) in entries.iter().enumerate() {
        index.entry(uuid_of(entry)).or_insert(position);
    }
    let Some(start) = chain.first().and_then(|head| index.get(uuid_of(head))).copied() else {
        return Ok(entries);
    };
    let kept: HashSet<&str> = chain.iter().map(uuid_of).collect();
    let mut by_uuid: HashMap<&str, &JsonObject> = HashMap::new();
    for entry in &entries {
        by_uuid.insert(uuid_of(entry), entry);
    }
    let mut live: HashSet<String> = HashSet::new();
    let seeds = std::iter::once(uuid_of(tail)).chain(kept.iter().copied());
    for seed in seeds {
        let mut current = by_uuid.get(seed).copied();
        while let Some(entry) = current {
            if !live.insert(uuid_of(entry).to_owned()) {
                break;
            }
            current = parent_uuid(entry).and_then(|parent| by_uuid.get(parent).copied());
        }
    }
    let mut pruning = Pruning {
        entries: &entries,
        by_uuid,
        live,
        index,
        start,
        left_out: HashSet::new(),
    };
    for entry in &entries {
        if is_boundary(entry)
            && pruning.reaches(uuid_of(entry))
            && !preserved_uuids(entry).iter().any(|uuid| pruning.live.contains(uuid))
        {
            pruning.leave_out(uuid_of(entry));
        }
    }
    loop {
        let remaining: Vec<JsonObject> = cleared
            .iter()
            .filter(|entry| !pruning.left_out.contains(uuid_of(entry)))
            .cloned()
            .collect();
        let linked = link(&remaining)?;
        let (length, same) = linked.as_ref().map_or((0, false), |linked| {
            (
                linked.chain.len(),
                linked.chain.iter().all(|entry| kept.contains(uuid_of(entry))),
            )
        });
        if length == kept.len() && same {
            break;
        }
        let Some(terminal) = linked.and_then(|linked| linked.terminal) else {
            break;
        };
        if !pruning.reaches(&terminal) {
            break;
        }
        pruning.leave_out(&terminal);
    }
    if pruning.left_out.is_empty() {
        return Ok(entries);
    }
    let left_out = pruning.left_out;
    Ok(entries
        .into_iter()
        .filter(|entry| !left_out.contains(uuid_of(entry)))
        .collect())
}

/// `XT`: the index of the entry with this uuid, else of the first queued command it was delivered from.
fn locate(entries: &[&JsonObject], target: &str) -> Option<usize> {
    entries
        .iter()
        .position(|entry| str_field(entry, "uuid") == Some(target))
        .or_else(|| {
            entries
                .iter()
                .position(|entry| command_string(entry, "source_uuid") == Some(target))
        })
}

/// `WMe`: the attachment of a deferred-tools record with its announced uuids renamed.
fn rename_announcements(entry: &JsonObject, ids: &HashMap<String, String>) -> Option<Value> {
    if kind_of(entry) != Some("attachment") {
        return None;
    }
    let attachment = entry.get("attachment")?.as_object()?;
    if str_field(attachment, "type") != Some("deferred_tools_record") {
        return None;
    }
    let announced = attachment.get("nameOnlyAnnouncements")?.as_array()?;
    let renamed: Vec<Value> = announced
        .iter()
        .filter_map(Value::as_str)
        .filter_map(|name| ids.get(name))
        .map(|id| string_value(id))
        .collect();
    let mut copy = attachment.clone();
    copy.insert("nameOnlyAnnouncements".to_owned(), Value::Array(renamed));
    Some(Value::Object(copy))
}

/// `z$`: compaction metadata with its preserved uuids renamed and `runningTurnSends` dropped.
fn rename_compaction(entry: &JsonObject, ids: &HashMap<&str, &str>) -> Option<Value> {
    if kind_of(entry) != Some("system") {
        return None;
    }
    let meta = entry.get("compactMetadata")?.as_object()?;
    let touched = ["preservedMessages", "preservedSegment", "runningTurnSends"];
    if !touched.iter().any(|key| meta.contains_key(*key)) {
        return None;
    }
    let rename = |value: &Value| match value {
        Value::String(text) => ids
            .get(text.as_str())
            .map_or_else(|| value.clone(), |id| string_value(id)),
        other => other.clone(),
    };
    let rename_keys = |value: &Value, keys: &[&str]| {
        let Value::Object(object) = value else {
            return value.clone();
        };
        let mut copy = object.clone();
        for key in keys {
            let Some(current) = object.get(*key) else { continue };
            let renamed = match current {
                Value::Array(items) => Value::Array(items.iter().map(rename).collect()),
                other => rename(other),
            };
            copy.insert((*key).to_owned(), renamed);
        }
        Value::Object(copy)
    };
    let mut out = JsonObject::new();
    for (key, value) in meta {
        if !touched.contains(&key.as_str()) {
            out.insert(key.clone(), value.clone());
        }
    }
    if let Some(messages) = meta.get("preservedMessages") {
        out.insert(
            "preservedMessages".to_owned(),
            rename_keys(messages, &TS_FORK_MESSAGE_KEYS),
        );
    }
    if let Some(segment) = meta.get("preservedSegment") {
        out.insert(
            "preservedSegment".to_owned(),
            rename_keys(segment, &TS_FORK_SEGMENT_KEYS),
        );
    }
    Some(Value::Object(out))
}

/// `NB` arguments beyond the state.
pub(crate) struct ForkTsRequest<'a> {
    pub(crate) session_id: &'a str,
    pub(crate) up_to_message_id: Option<&'a str>,
    pub(crate) title: Option<&'a str>,
}

/// The entries of the forked transcript and the id of the new session.
#[derive(Debug, Clone)]
pub(crate) struct ForkedTranscript {
    pub(crate) entries: Vec<JsonObject>,
    pub(crate) session_id: String,
}

/// The slice of the transcript a fork copies: the main-thread entries, trimmed at `target` when it is set.
fn fork_slice(
    state: &TranscriptState,
    session_id: &str,
    target: Option<&str>,
    ids: &mut dyn FnMut() -> String,
) -> Result<Vec<JsonObject>, ForkTsError> {
    let slice: Vec<JsonObject> = state
        .transcript
        .iter()
        .filter(|entry| !flag(entry, "isSidechain"))
        .cloned()
        .collect();
    if slice.is_empty() {
        return Err(ForkTsError::NoMessages(session_id.to_owned()));
    }
    let Some(target) = target.filter(|target| !target.is_empty()) else {
        return Ok(slice);
    };
    let chain = link(&slice)?.map(|linked| linked.chain).unwrap_or_default();
    let mut first: HashMap<&str, usize> = HashMap::new();
    for (position, entry) in slice.iter().enumerate() {
        first.entry(uuid_of(entry)).or_insert(position);
    }
    let last = chain
        .iter()
        .map(|entry| first.get(uuid_of(entry)).map_or(-1, |index| *index as i64))
        .fold(-1_i64, i64::max);
    let mut combined: Vec<&JsonObject> = chain.iter().collect();
    combined.extend(slice.iter().skip(usize::try_from(last + 1).unwrap_or(0)));
    let found = locate(&combined, target).map(|index| combined[index]);
    let cut = match found {
        Some(entry) => first.get(uuid_of(entry)).map_or(-1, |index| *index as i64),
        None => {
            let plain: Vec<&JsonObject> = slice.iter().collect();
            locate(&plain, target).map_or(-1, |index| index as i64)
        }
    };
    let Ok(cut) = usize::try_from(cut) else {
        return Err(ForkTsError::MessageNotFound {
            message_id: target.to_owned(),
            session_id: session_id.to_owned(),
        });
    };
    let cut_uuid = uuid_of(&slice[cut]).to_owned();
    let trimmed = clear_preserved(&cut_uuid, compact_prefix(&slice[..=cut]));
    prune_dead_branches(trimmed, ids)
}

/// `NB`: the entries of a fork of the transcript described by `state`.
///
/// `ids` stands for `crypto.randomUUID`, `now_ms` for `Date.now`, and `derive_title` for the title derivation the
/// caller runs only when the request has no usable title.
pub(crate) fn fork_entries(
    state: &TranscriptState,
    request: &ForkTsRequest<'_>,
    derive_title: impl FnOnce() -> String,
    ids: &mut dyn FnMut() -> String,
    now_ms: &mut dyn FnMut() -> i64,
) -> Result<ForkedTranscript, ForkTsError> {
    let session = request.session_id;
    let slice = fork_slice(state, session, request.up_to_message_id, ids)?;
    let mut renamed: HashMap<String, String> = HashMap::new();
    for entry in &slice {
        renamed.insert(uuid_of(entry).to_owned(), ids());
    }
    let kept: Vec<&JsonObject> = slice
        .iter()
        .filter(|entry| kind_of(entry) != Some("progress"))
        .collect();
    if kept.is_empty() {
        return Err(ForkTsError::NoMessages(session.to_owned()));
    }
    let kept_ids: HashMap<&str, &str> = kept
        .iter()
        .filter_map(|entry| renamed.get(uuid_of(entry)).map(|id| (uuid_of(entry), id.as_str())))
        .collect();
    let mut by_uuid: HashMap<&str, &JsonObject> = HashMap::new();
    for entry in &slice {
        by_uuid.insert(uuid_of(entry), entry);
    }
    let new_session = ids();
    let stamp = iso_from_epoch_ms(now_ms());
    let mut out: Vec<JsonObject> = Vec::new();
    if state.history_suppressed {
        out.push(object([
            ("type", string_value("history-suppression")),
            ("sessionId", string_value(&new_session)),
            ("cause", string_value("fork_inherit")),
            ("ts", Value::String(iso_from_epoch_ms(now_ms()))),
        ]));
    }
    for (position, entry) in kept.iter().enumerate() {
        let new_uuid = renamed.get(uuid_of(entry)).cloned().unwrap_or_default();
        let mut parent = Value::Null;
        let mut cursor = parent_uuid(entry);
        let mut seen: Option<HashSet<&str>> = None;
        while let Some(id) = cursor {
            let Some(ancestor) = by_uuid.get(id).copied() else {
                break;
            };
            if kind_of(ancestor) != Some("progress") {
                parent = renamed.get(id).map_or(Value::Null, |new| string_value(new));
                break;
            }
            let visited = seen.get_or_insert_with(HashSet::new);
            if visited.contains(uuid_of(ancestor)) {
                parent = renamed
                    .get(uuid_of(ancestor))
                    .map_or(Value::Null, |new| string_value(new));
                break;
            }
            visited.insert(uuid_of(ancestor));
            cursor = parent_uuid(ancestor);
        }
        let timestamp = if position + 1 == kept.len() {
            Some(string_value(&stamp))
        } else {
            entry.get("timestamp").cloned()
        };
        let logical = match entry.get("logicalParentUuid") {
            None => None,
            Some(Value::Null) => Some(Value::Null),
            Some(other) => Some(
                other
                    .as_str()
                    .and_then(|id| renamed.get(id))
                    .map_or(Value::Null, |new| string_value(new)),
            ),
        };
        let mut copy = (*entry).clone();
        if kind_of(entry) == Some("system") && str_field(entry, "subtype") == Some("model_refusal_fallback") {
            copy.insert("neutralizedByFork".to_owned(), Value::Bool(true));
        }
        if let Some(attachment) = rename_announcements(entry, &renamed) {
            copy.insert("attachment".to_owned(), attachment);
        }
        if let Some(meta) = rename_compaction(entry, &kept_ids) {
            copy.insert("compactMetadata".to_owned(), meta);
        }
        if let Some(source) = command_string(entry, "source_uuid").and_then(|id| renamed.get(id)) {
            if kind_of(entry) == Some("attachment") {
                let mut attachment = entry
                    .get("attachment")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                attachment.insert("source_uuid".to_owned(), string_value(source));
                copy.insert("attachment".to_owned(), Value::Object(attachment));
            } else {
                let mut fold = entry
                    .get("fold")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                let mut command = queued_command(entry).cloned().unwrap_or_default();
                command.insert("source_uuid".to_owned(), string_value(source));
                fold.insert("command".to_owned(), Value::Object(command));
                copy.insert("fold".to_owned(), Value::Object(fold));
            }
        }
        copy.insert("uuid".to_owned(), Value::String(new_uuid));
        copy.insert("parentUuid".to_owned(), parent);
        if let Some(logical) = logical {
            copy.insert("logicalParentUuid".to_owned(), logical);
        }
        copy.insert("sessionId".to_owned(), string_value(&new_session));
        if let Some(timestamp) = timestamp {
            copy.insert("timestamp".to_owned(), timestamp);
        }
        let mut copy = clear_fork_keys(&copy);
        copy.insert(
            "forkedFrom".to_owned(),
            Value::Object(object([
                ("sessionId", string_value(session)),
                ("messageUuid", entry.get("uuid").cloned().unwrap_or(Value::Null)),
            ])),
        );
        out.push(copy);
    }
    if !state.replacements.is_empty() {
        out.push(object([
            ("type", string_value("content-replacement")),
            ("sessionId", string_value(&new_session)),
            (
                "replacements",
                Value::Array(state.replacements.iter().cloned().map(Value::Object).collect()),
            ),
            ("uuid", Value::String(ids())),
            ("timestamp", string_value(&stamp)),
        ]));
    }
    if let Some(atis) = &state.atis_latch {
        out.push(object([
            ("type", string_value("atis-latch")),
            ("sessionId", string_value(&new_session)),
            ("atis", string_value(atis)),
        ]));
    }
    if let Some(tools) = &state.tools_used {
        out.push(object([
            ("type", string_value("tools-used")),
            ("sessionId", string_value(&new_session)),
            (
                "tools",
                Value::Array(tools.iter().map(|tool| string_value(tool)).collect()),
            ),
        ]));
    }
    if let Some(flow) = &state.data_flow {
        out.push(object([
            ("type", string_value("data-flow-records")),
            ("v", Value::from(TS_FLOW_VERSION)),
            ("sessionId", string_value(&new_session)),
            (
                "records",
                Value::Array(flow.records.iter().cloned().map(Value::Object).collect()),
            ),
            ("snapshot", Value::Bool(true)),
        ]));
        if flow.incomplete {
            out.push(object([
                ("type", string_value("data-flow-incomplete")),
                ("v", Value::from(TS_FLOW_VERSION)),
                ("sessionId", string_value(&new_session)),
            ]));
        }
    }
    if state.data_flow.is_some() || state.derived_from.is_some() {
        let mut sessions = dedupe_derived(state.derived_from.clone().unwrap_or_default());
        let mut known: HashSet<Option<String>> = sessions.iter().map(|item| item.id.clone()).collect();
        for entry in &state.transcript {
            let origin = entry
                .get("forkedFrom")
                .and_then(Value::as_object)
                .and_then(|from| identifier(from.get("sessionId")));
            if let Some(origin) = origin
                && known.insert(Some(origin.to_owned()))
            {
                sessions.push(Derived::session(origin, "fork"));
            }
        }
        sessions.push(Derived::session(session, "fork"));
        out.push(object([
            ("type", string_value("derived-from")),
            ("v", Value::from(TS_FLOW_VERSION)),
            ("sessionId", string_value(&new_session)),
            (
                "sessions",
                Value::Array(dedupe_derived(sessions).iter().map(Derived::to_value).collect()),
            ),
        ]));
    }
    if let Some(cwd) = state.relocated_cwd.as_deref().filter(|cwd| !cwd.is_empty()) {
        out.push(object([
            ("type", string_value("relocated")),
            ("sessionId", string_value(&new_session)),
            ("relocatedCwd", string_value(cwd)),
        ]));
    }
    let mut title = request.title.map(js_trim).unwrap_or("").to_owned();
    if title.is_empty() {
        let derived = derive_title();
        let base = if derived.is_empty() {
            TS_FORK_TITLE_FALLBACK
        } else {
            derived.as_str()
        };
        title = format!("{base} (fork)");
    }
    out.push(object([
        ("type", string_value("custom-title")),
        ("sessionId", string_value(&new_session)),
        ("customTitle", Value::String(title)),
        ("uuid", Value::String(ids())),
        ("timestamp", string_value(&stamp)),
    ]));
    Ok(ForkedTranscript {
        entries: out,
        session_id: new_session,
    })
}

/// The transcript file body: one `JSON.stringify` line per entry.
pub(crate) fn serialize_entries(entries: &[JsonObject]) -> String {
    let mut text = String::new();
    for entry in entries {
        text.push_str(&js_stringify(&Value::Object(entry.clone())));
        text.push('\n');
    }
    text
}

/// `JSON.stringify`: integral floats print without a fraction and numbers follow `Number::toString`.
pub(crate) fn js_stringify(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => js_number(number),
        Value::String(text) => serde_json::to_string(text).unwrap_or_default(),
        Value::Array(items) => format!("[{}]", items.iter().map(js_stringify).collect::<Vec<_>>().join(",")),
        Value::Object(map) => format!(
            "{{{}}}",
            map.iter()
                .map(|(key, item)| format!(
                    "{}:{}",
                    serde_json::to_string(key).unwrap_or_default(),
                    js_stringify(item)
                ))
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

fn js_number(number: &serde_json::Number) -> String {
    const EXACT: i64 = 9_007_199_254_740_992;
    if let Some(integer) = number.as_i64()
        && integer.abs() <= EXACT
    {
        return integer.to_string();
    }
    if let Some(integer) = number.as_u64()
        && integer <= EXACT as u64
    {
        return integer.to_string();
    }
    number.as_f64().map_or_else(|| "null".to_owned(), js_float)
}

/// `Number.prototype.toString` for a finite double.
fn js_float(value: f64) -> String {
    if !value.is_finite() {
        return "null".to_owned();
    }
    if value == 0.0 {
        return "0".to_owned();
    }
    let sign = if value < 0.0 { "-" } else { "" };
    let scientific = format!("{:e}", value.abs());
    let Some((mantissa, exponent)) = scientific.split_once('e') else {
        return scientific;
    };
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let point: i32 = exponent.parse::<i32>().map_or(0, |exponent| exponent + 1);
    let count = digits.len() as i32;
    let body = if count <= point && point <= 21 {
        format!("{digits}{}", "0".repeat((point - count) as usize))
    } else if 0 < point && point <= 21 {
        format!("{}.{}", &digits[..point as usize], &digits[point as usize..])
    } else if -6 < point && point <= 0 {
        format!("0.{}{digits}", "0".repeat((-point) as usize))
    } else {
        let power = point - 1;
        let sign_char = if power < 0 { '-' } else { '+' };
        if count == 1 {
            format!("{digits}e{sign_char}{}", power.abs())
        } else {
            format!("{}.{}e{sign_char}{}", &digits[..1], &digits[1..], power.abs())
        }
    };
    format!("{sign}{body}")
}

#[cfg(test)]
#[path = "session_fork_ts_tests.rs"]
mod tests;
