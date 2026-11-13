//! Transcript reconstruction shared by the filesystem and store-backed session readers.
//!
//! Ports upstream `_parse_transcript_entries`, `_build_conversation_chain`,
//! `_recover_orphaned_parallel_tool_results`, `_build_subagent_chain` and the paging rules.

use crate::constants::TRANSCRIPT_TYPES;
use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::JsonObject;

/// Which side of the conversation a [`SessionMessage`] came from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionMessageKind {
    User,
    Assistant,
    /// Only returned by the TypeScript dialect with `include_system_messages`.
    System,
}

/// Fields the TypeScript SDK adds to a message beyond the Python shape. All stay at their default in the Python
/// dialect.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MessageExtras {
    pub timestamp: Option<Value>,
    pub origin: Option<Value>,
    /// `is_meta` upstream: a meta, compact summary or transcript-only entry.
    pub is_meta: bool,
    pub is_compact_summary: bool,
    pub is_queued_command: bool,
    pub is_completed_local_command: bool,
    pub interrupted_by_shutdown: bool,
    pub tool_denial_unanswered: Option<String>,
    pub tool_result_meta: Option<Value>,
}

/// A user or assistant message from a session transcript (`SessionMessage` upstream).
///
/// The two parent fields are only set for subagent transcripts, from the `agent-<id>.meta.json` sidecar.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionMessage {
    pub kind: SessionMessageKind,
    pub uuid: String,
    pub session_id: String,
    /// Raw Anthropic API message object, passed through unchanged.
    pub message: Value,
    pub parent_tool_use_id: Option<String>,
    pub parent_agent_id: Option<String>,
    pub extras: MessageExtras,
}

/// Paging shared by the message readers: `limit` of `None` or `Some(0)` returns everything, as upstream does.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MessagePage {
    pub limit: Option<usize>,
    pub offset: usize,
}

impl MessagePage {
    pub(crate) fn apply<T>(self, mut items: Vec<T>) -> Vec<T> {
        match self.limit {
            Some(limit) if limit > 0 => items.into_iter().skip(self.offset).take(limit).collect(),
            _ if self.offset > 0 => items.drain(self.offset.min(items.len())..).collect(),
            _ => items,
        }
    }
}

/// Whether an entry is a transcript message: a known type and a string uuid.
pub(crate) fn is_transcript_entry(entry: &JsonObject) -> bool {
    entry
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|kind| TRANSCRIPT_TYPES.contains(&kind))
        && entry.get("uuid").is_some_and(Value::is_string)
}

/// Parses JSONL into the entries that carry a `uuid` and a transcript message type, skipping corrupt lines.
pub(crate) fn parse_transcript_entries(content: &str) -> Vec<JsonObject> {
    content
        .split('\n')
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            match serde_json::from_str::<Value>(line).ok()? {
                Value::Object(entry) if is_transcript_entry(&entry) => Some(entry),
                _ => None,
            }
        })
        .collect()
}

fn uuid_of(entry: &JsonObject) -> &str {
    entry.get("uuid").and_then(Value::as_str).unwrap_or("")
}

fn parent_of(entry: &JsonObject) -> Option<&str> {
    entry
        .get("parentUuid")
        .and_then(Value::as_str)
        .filter(|parent| !parent.is_empty())
}

fn kind_of(entry: &JsonObject) -> Option<&str> {
    entry.get("type").and_then(Value::as_str)
}

/// Python truthiness of a JSON value, which upstream uses for `isSidechain`, `teamName` and `isMeta`.
pub(crate) fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Number(number)) => number.as_f64().is_some_and(|number| number != 0.0),
        Some(Value::String(text)) => !text.is_empty(),
        Some(Value::Array(items)) => !items.is_empty(),
        Some(Value::Object(fields)) => !fields.is_empty(),
    }
}

/// Entries addressed by position, with a later duplicate `uuid` replacing the earlier entry in the lookup while the
/// earlier position keeps its place in iteration order, exactly like the Python `dict` upstream builds.
struct Index<'a> {
    entries: &'a [JsonObject],
    last: HashMap<&'a str, usize>,
    first_order: Vec<&'a str>,
}

impl<'a> Index<'a> {
    fn new(entries: &'a [JsonObject]) -> Self {
        let mut last = HashMap::new();
        let mut first_order = Vec::new();
        for (position, entry) in entries.iter().enumerate() {
            let uuid = uuid_of(entry);
            if last.insert(uuid, position).is_none() {
                first_order.push(uuid);
            }
        }
        Self {
            entries,
            last,
            first_order,
        }
    }

    fn get(&self, uuid: &str) -> Option<usize> {
        self.last.get(uuid).copied()
    }

    fn values(&self) -> impl Iterator<Item = usize> + '_ {
        self.first_order.iter().filter_map(|uuid| self.get(uuid))
    }

    fn entry(&self, position: usize) -> &'a JsonObject {
        &self.entries[position]
    }
}

fn walk_to_root(index: &Index<'_>, start: usize) -> (Vec<usize>, HashSet<String>) {
    let mut chain = Vec::new();
    let mut seen = HashSet::new();
    let mut current = Some(start);
    while let Some(position) = current {
        let entry = index.entry(position);
        if !seen.insert(uuid_of(entry).to_owned()) {
            break;
        }
        chain.push(position);
        current = parent_of(entry).and_then(|parent| index.get(parent));
    }
    chain.reverse();
    (chain, seen)
}

/// Builds the main conversation chain: the most recent main-chain leaf walked back through `parentUuid`, with the
/// tool calls and results a parallel turn forked off the walk put back. `logicalParentUuid` is deliberately not
/// followed, matching upstream.
fn build_conversation_chain(entries: &[JsonObject]) -> Vec<usize> {
    if entries.is_empty() {
        return Vec::new();
    }
    let index = Index::new(entries);
    let mut entry_position: HashMap<&str, usize> = HashMap::new();
    for (position, entry) in entries.iter().enumerate() {
        entry_position.insert(uuid_of(entry), position);
    }
    let parents: HashSet<&str> = entries
        .iter()
        .filter_map(|entry| {
            entry
                .get("parentUuid")
                .and_then(Value::as_str)
                .filter(|parent| !parent.is_empty())
        })
        .collect();

    let mut leaves = Vec::new();
    for (terminal, entry) in entries.iter().enumerate() {
        if parents.contains(uuid_of(entry)) {
            continue;
        }
        let mut seen = HashSet::new();
        let mut current = Some(terminal);
        while let Some(position) = current {
            let walked = index.entry(position);
            if !seen.insert(uuid_of(walked)) {
                break;
            }
            if matches!(kind_of(walked), Some("user" | "assistant")) {
                leaves.push(position);
                break;
            }
            current = parent_of(walked).and_then(|parent| index.get(parent));
        }
    }
    if leaves.is_empty() {
        return Vec::new();
    }
    let main_leaves: Vec<usize> = leaves
        .iter()
        .copied()
        .filter(|position| {
            let leaf = index.entry(*position);
            !truthy(leaf.get("isSidechain")) && !truthy(leaf.get("teamName")) && !truthy(leaf.get("isMeta"))
        })
        .collect();
    let candidates = if main_leaves.is_empty() { &leaves } else { &main_leaves };
    let rank = |position: usize| entry_position.get(uuid_of(index.entry(position))).copied().unwrap_or(0);
    let mut best = candidates[0];
    for &candidate in &candidates[1..] {
        if rank(candidate) > rank(best) {
            best = candidate;
        }
    }

    let (chain, seen) = walk_to_root(&index, best);
    recover_orphaned_parallel_tool_results(&index, chain, seen)
}

fn assistant_message_id(entry: &JsonObject) -> Option<&str> {
    if kind_of(entry) != Some("assistant") {
        return None;
    }
    entry
        .get("message")?
        .as_object()?
        .get("id")?
        .as_str()
        .filter(|id| !id.is_empty())
}

fn content_block_ids<'a>(entry: &'a JsonObject, block_type: &str, id_field: &str) -> Vec<&'a str> {
    let Some(content) = entry
        .get("message")
        .and_then(Value::as_object)
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    content
        .iter()
        .filter_map(Value::as_object)
        .filter(|block| block.get("type").and_then(Value::as_str) == Some(block_type))
        .filter_map(|block| block.get(id_field).and_then(Value::as_str))
        .collect()
}

fn is_tool_result_entry(entry: &JsonObject) -> bool {
    if kind_of(entry) != Some("user") || parent_of(entry).is_none() {
        return false;
    }
    entry
        .get("message")
        .and_then(Value::as_object)
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
        .is_some_and(|content| {
            content.iter().any(|block| {
                block
                    .as_object()
                    .and_then(|block| block.get("type"))
                    .and_then(Value::as_str)
                    == Some("tool_result")
            })
        })
}

fn same_writer(a: &JsonObject, b: &JsonObject) -> bool {
    truthy(a.get("isSidechain")) == truthy(b.get("isSidechain")) && a.get("agentId") == b.get("agentId")
}

fn timestamp_key(entry: &JsonObject) -> &str {
    entry.get("timestamp").and_then(Value::as_str).unwrap_or("")
}

/// Puts back the assistant entries and tool results a parallel-tool turn left off the `parentUuid` walk, directly
/// after the last on-chain entry of the same response, so every result follows its call.
fn recover_orphaned_parallel_tool_results(
    index: &Index<'_>,
    chain: Vec<usize>,
    mut seen: HashSet<String>,
) -> Vec<usize> {
    let chain_assistants: Vec<usize> = chain
        .iter()
        .copied()
        .filter(|position| kind_of(index.entry(*position)) == Some("assistant"))
        .collect();
    if chain_assistants.is_empty() {
        return chain;
    }
    let mut anchor_by_message_id: HashMap<&str, usize> = HashMap::new();
    for &position in &chain_assistants {
        if let Some(message_id) = assistant_message_id(index.entry(position)) {
            anchor_by_message_id.insert(message_id, position);
        }
    }
    if anchor_by_message_id.is_empty() {
        return chain;
    }

    let mut siblings_by_message_id: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut call_owner: HashMap<&str, Option<usize>> = HashMap::new();
    let mut tool_result_entries = Vec::new();
    for position in index.values() {
        let entry = index.entry(position);
        if let Some(message_id) = assistant_message_id(entry) {
            siblings_by_message_id.entry(message_id).or_default().push(position);
            for call_id in content_block_ids(entry, "tool_use", "id") {
                match call_owner.get(call_id).copied() {
                    None => {
                        call_owner.insert(call_id, Some(position));
                    }
                    Some(owner) => {
                        let same_response =
                            owner.is_some_and(|owner| assistant_message_id(index.entry(owner)) == Some(message_id));
                        call_owner.insert(call_id, if same_response { Some(position) } else { None });
                    }
                }
            }
        } else if is_tool_result_entry(entry) {
            tool_result_entries.push(position);
        }
    }

    let mut results_by_assistant: HashMap<&str, Vec<usize>> = HashMap::new();
    for &position in &tool_result_entries {
        let result = index.entry(position);
        if let Some(parent) = parent_of(result) {
            results_by_assistant.entry(parent).or_default().push(position);
        }
        if let Some(source) = result.get("sourceToolAssistantUUID").and_then(Value::as_str)
            && let Some(source_position) = index.get(source)
            && same_writer(result, index.entry(source_position))
        {
            results_by_assistant.entry(source).or_default().push(position);
        }
        for call_id in content_block_ids(result, "tool_result", "tool_use_id") {
            if let Some(Some(owner)) = call_owner.get(call_id).copied()
                && same_writer(result, index.entry(owner))
            {
                results_by_assistant
                    .entry(uuid_of(index.entry(owner)))
                    .or_default()
                    .push(position);
            }
        }
    }

    let mut answered_on_chain: Option<HashSet<&str>> = None;
    let mut file_position: Option<HashMap<&str, usize>> = None;
    let mut processed: HashSet<&str> = HashSet::new();
    let mut inserts: HashMap<String, Vec<usize>> = HashMap::new();
    for &assistant in &chain_assistants {
        let Some(message_id) = assistant_message_id(index.entry(assistant)) else {
            continue;
        };
        if !processed.insert(message_id) {
            continue;
        }
        let group = siblings_by_message_id
            .get(message_id)
            .cloned()
            .unwrap_or_else(|| vec![assistant]);
        let member_uuids: HashSet<&str> = group.iter().map(|member| uuid_of(index.entry(*member))).collect();
        let mut orphaned_siblings: Vec<usize> = group
            .iter()
            .copied()
            .filter(|member| !seen.contains(uuid_of(index.entry(*member))))
            .collect();
        let mut orphaned_results: Vec<usize> = Vec::new();
        let mut strays: Vec<usize> = Vec::new();
        let mut collected: HashSet<&str> = HashSet::new();
        for &member in &group {
            let member_uuid = uuid_of(index.entry(member));
            for &result_position in results_by_assistant.get(member_uuid).map(Vec::as_slice).unwrap_or(&[]) {
                let result = index.entry(result_position);
                let result_uuid = uuid_of(result);
                if seen.contains(result_uuid) || collected.contains(result_uuid) {
                    continue;
                }
                collected.insert(result_uuid);
                if parent_of(result).is_some_and(|parent| member_uuids.contains(parent)) {
                    orphaned_results.push(result_position);
                } else {
                    strays.push(result_position);
                }
            }
        }

        if !strays.is_empty() {
            let on_chain = answered_on_chain.get_or_insert_with(|| {
                chain
                    .iter()
                    .flat_map(|position| content_block_ids(index.entry(*position), "tool_result", "tool_use_id"))
                    .collect()
            });
            let positions = file_position.get_or_insert_with(|| {
                index
                    .first_order
                    .iter()
                    .enumerate()
                    .map(|(position, uuid)| (*uuid, position))
                    .collect()
            });
            let mut answered: HashSet<&str> = on_chain.clone();
            for &result in &orphaned_results {
                answered.extend(content_block_ids(index.entry(result), "tool_result", "tool_use_id"));
            }
            strays.sort_by_key(|position| positions.get(uuid_of(index.entry(*position))).copied().unwrap_or(0));
            for result_position in strays {
                let call_ids = content_block_ids(index.entry(result_position), "tool_result", "tool_use_id");
                let answers_open_call = call_ids.iter().any(|call_id| {
                    !answered.contains(call_id)
                        && matches!(call_owner.get(call_id).copied().flatten(),
                            Some(owner) if member_uuids.contains(uuid_of(index.entry(owner))))
                });
                if !answers_open_call {
                    continue;
                }
                answered.extend(call_ids);
                orphaned_results.push(result_position);
            }
        }

        if orphaned_siblings.is_empty() && orphaned_results.is_empty() {
            continue;
        }
        orphaned_siblings.sort_by(|a, b| timestamp_key(index.entry(*a)).cmp(timestamp_key(index.entry(*b))));
        orphaned_results.sort_by(|a, b| timestamp_key(index.entry(*a)).cmp(timestamp_key(index.entry(*b))));
        let recovered: Vec<usize> = orphaned_siblings.into_iter().chain(orphaned_results).collect();
        for &position in &recovered {
            seen.insert(uuid_of(index.entry(position)).to_owned());
        }
        if let Some(anchor) = anchor_by_message_id.get(message_id) {
            inserts.insert(uuid_of(index.entry(*anchor)).to_owned(), recovered);
        }
    }
    if inserts.is_empty() {
        return chain;
    }
    let mut result = Vec::new();
    for position in chain {
        result.push(position);
        if let Some(recovered) = inserts.get(uuid_of(index.entry(position))) {
            result.extend(recovered);
        }
    }
    result
}

fn is_visible_message(entry: &JsonObject) -> bool {
    matches!(kind_of(entry), Some("user" | "assistant"))
        && !truthy(entry.get("isMeta"))
        && !truthy(entry.get("isSidechain"))
        && !truthy(entry.get("teamName"))
}

fn to_session_message(
    entry: &JsonObject,
    parent_tool_use_id: Option<&str>,
    parent_agent_id: Option<&str>,
) -> SessionMessage {
    SessionMessage {
        kind: if kind_of(entry) == Some("user") {
            SessionMessageKind::User
        } else {
            SessionMessageKind::Assistant
        },
        uuid: uuid_of(entry).to_owned(),
        session_id: entry.get("sessionId").and_then(Value::as_str).unwrap_or("").to_owned(),
        message: entry.get("message").cloned().unwrap_or(Value::Null),
        parent_tool_use_id: parent_tool_use_id.map(str::to_owned),
        parent_agent_id: parent_agent_id.map(str::to_owned),
        extras: MessageExtras::default(),
    }
}

/// Builds the visible conversation of a main session from parsed entries and applies paging.
pub(crate) fn entries_to_session_messages(entries: &[JsonObject], page: MessagePage) -> Vec<SessionMessage> {
    let messages = build_conversation_chain(entries)
        .into_iter()
        .map(|position| &entries[position])
        .filter(|entry| is_visible_message(entry))
        .map(|entry| to_session_message(entry, None, None))
        .collect();
    page.apply(messages)
}

/// Builds a subagent conversation: the last user or assistant entry walked back to the root. Every message carries
/// the same parent ids.
pub(crate) fn entries_to_subagent_messages(
    entries: &[JsonObject],
    page: MessagePage,
    parent_tool_use_id: Option<&str>,
    parent_agent_id: Option<&str>,
) -> Vec<SessionMessage> {
    let Some(leaf) = entries
        .iter()
        .rposition(|entry| matches!(kind_of(entry), Some("user" | "assistant")))
    else {
        return Vec::new();
    };
    let index = Index::new(entries);
    let (chain, _) = walk_to_root(&index, leaf);
    let messages = chain
        .into_iter()
        .map(|position| &entries[position])
        .filter(|entry| matches!(kind_of(entry), Some("user" | "assistant")))
        .map(|entry| to_session_message(entry, parent_tool_use_id, parent_agent_id))
        .collect();
    page.apply(messages)
}

/// Extracts `(toolUseId, parentAgentId)` from a `.meta.json` sidecar or a synthetic `agent_metadata` entry.
pub(crate) fn parent_ids_from_agent_metadata(meta: Option<&JsonObject>) -> (Option<String>, Option<String>) {
    let field = |name: &str| {
        meta.and_then(|meta| meta.get(name))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    (field("toolUseId"), field("parentAgentId"))
}
