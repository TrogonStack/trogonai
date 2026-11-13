//! TypeScript SDK 0.3.323 `getSessionMessages` pipeline.
//!
//! The Python SDK walks the `parentUuid` chain from the last entry. TypeScript first re-links `compact_boundary`
//! preserved segments, picks a leaf with terminal and sidechain rules, regroups assistant fragments and tool results
//! (`ZIe`), converts delivered queued commands back to user messages (`iw`), appends unlinked attachment entries
//! (`nLe`) and finally filters and maps the entries (`tLe`/`aw`). [`ChainAlgorithm`] selects between the two.

use std::collections::{HashMap, HashSet};

use crate::constants::{TS_DENIAL_KINDS, TS_REPLY_PREFIXES};
use indexmap::IndexMap;
use serde_json::{Map, Value};

use crate::{
    JsonObject, MessagePage, SessionMessage, SessionMessageKind,
    session_transcript::{MessageExtras, is_transcript_entry, truthy},
};

/// Which upstream SDK's rules rebuild the visible conversation of a transcript.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ChainAlgorithm {
    /// TypeScript `getSessionMessages`.
    #[default]
    TypeScript,
    /// Python `get_session_messages`.
    Python,
}

/// Arguments of the message readers beyond paging.
#[derive(Clone, Copy, Debug, Default)]
pub struct MessageOptions {
    pub page: MessagePage,
    /// Include `system` entries (TypeScript only; Python never returns them).
    pub include_system_messages: bool,
    pub algorithm: ChainAlgorithm,
}

type Entries = IndexMap<String, JsonObject>;

fn str_field<'a>(entry: &'a JsonObject, key: &str) -> Option<&'a str> {
    entry.get(key).and_then(Value::as_str)
}

fn uuid_of(entry: &JsonObject) -> &str {
    str_field(entry, "uuid").unwrap_or("")
}

fn kind_of(entry: &JsonObject) -> Option<&str> {
    str_field(entry, "type")
}

/// A truthy `parentUuid`, the only kind JavaScript follows.
fn parent_uuid(entry: &JsonObject) -> Option<&str> {
    str_field(entry, "parentUuid").filter(|parent| !parent.is_empty())
}

fn is_conversation(entry: &JsonObject) -> bool {
    matches!(kind_of(entry), Some("user" | "assistant"))
}

fn flag(entry: &JsonObject, key: &str) -> bool {
    truthy(entry.get(key))
}

fn set_parent(entry: &mut JsonObject, parent: Option<&Value>) {
    match parent {
        Some(value) => {
            entry.insert("parentUuid".to_owned(), value.clone());
        }
        None => {
            entry.shift_remove("parentUuid");
        }
    }
}

/// `Hd`: a run id the CLI accepts.
fn valid_run_id(value: Option<&Value>) -> Option<&str> {
    value.and_then(Value::as_str).filter(|id| {
        (1..=128).contains(&id.len())
            && id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b':' | b'-'))
    })
}

/// `VA`: normalizes a task-notification origin, leaving every other origin alone.
fn normalized_origin(origin: &Value) -> Value {
    let Some(object) = origin.as_object() else {
        return origin.clone();
    };
    if object.get("kind").and_then(Value::as_str) != Some("task-notification") {
        return origin.clone();
    }
    let mut out = Map::new();
    out.insert("kind".to_owned(), Value::from("task-notification"));
    for key in ["subkind", "fireReason", "producer"] {
        if let Some(value) = object.get(key) {
            out.insert(key.to_owned(), value.clone());
        }
    }
    if let Some(run) = valid_run_id(object.get("runId")) {
        out.insert("runId".to_owned(), Value::from(run));
    }
    if object.get("taskKind").and_then(Value::as_str) == Some("mcp-task") {
        out.insert("taskKind".to_owned(), Value::from("mcp-task"));
    }
    Value::Object(out)
}

/// A JSON object from ordered pairs.
fn object<const N: usize>(pairs: [(&str, Value); N]) -> Value {
    Value::Object(pairs.into_iter().map(|(key, value)| (key.to_owned(), value)).collect())
}

/// `FU`: the origin of a queued command, adding the run id of a task notification.
fn command_origin(origin: Option<&Value>, mode: Option<&str>, run_id: Option<&Value>) -> Option<Value> {
    let normalized = origin
        .map(normalized_origin)
        .filter(|origin| !origin.is_null())
        .or_else(|| (mode == Some("task-notification")).then(|| object([("kind", Value::from("task-notification"))])));
    let run = valid_run_id(run_id);
    match (normalized, run) {
        (Some(Value::Object(mut object)), Some(run))
            if object.get("kind").and_then(Value::as_str) == Some("task-notification")
                && !object.contains_key("runId") =>
        {
            object.insert("runId".to_owned(), Value::from(run));
            Some(Value::Object(object))
        }
        (other, _) => other,
    }
}

/// `$d`: a voice call line recorded as a system entry is a user message or a queued command.
fn fold_voice_call(entry: JsonObject) -> JsonObject {
    if kind_of(&entry) != Some("system") || str_field(&entry, "subtype") != Some("voice_call_talk") {
        return entry;
    }
    let mut rest = entry.clone();
    for key in ["subtype", "level", "msgV", "content", "blocks", "fold"] {
        rest.shift_remove(key);
    }
    let Some(content) = entry.get("content").and_then(Value::as_str) else {
        return entry;
    };
    let voice = rest
        .get("origin")
        .and_then(Value::as_object)
        .is_some_and(|origin| origin.get("kind").and_then(Value::as_str) == Some("voice-call"));
    if !voice {
        return entry;
    }
    let body = match entry.get("blocks") {
        Some(blocks @ Value::Array(_)) => blocks.clone(),
        _ => Value::from(content),
    };
    let Some(fold) = entry.get("fold") else {
        rest.insert("type".to_owned(), Value::from("user"));
        rest.insert(
            "message".to_owned(),
            object([("role", Value::from("user")), ("content", body)]),
        );
        return rest;
    };
    let Some(fold) = fold.as_object() else {
        return entry;
    };
    let Some(command) = fold.get("command").and_then(Value::as_object) else {
        return entry;
    };
    let origin = rest.shift_remove("origin");
    rest.shift_remove("isMeta");
    let mut out: JsonObject = fold.clone();
    out.shift_remove("command");
    for (key, value) in rest {
        out.insert(key, value);
    }
    out.insert("type".to_owned(), Value::from("attachment"));
    let mut attachment = command.clone();
    attachment.insert("type".to_owned(), Value::from("queued_command"));
    attachment.insert("prompt".to_owned(), body);
    if let Some(origin) = origin {
        attachment.insert("origin".to_owned(), origin);
    }
    out.insert("attachment".to_owned(), Value::Object(attachment));
    out
}

/// `U$`: the queued command an entry carries.
fn queued_command(entry: &JsonObject) -> Option<&JsonObject> {
    if kind_of(entry) == Some("system") && str_field(entry, "subtype") == Some("voice_call_talk") {
        return entry
            .get("fold")
            .and_then(Value::as_object)
            .and_then(|fold| fold.get("command"))
            .and_then(Value::as_object);
    }
    if kind_of(entry) != Some("attachment") {
        return None;
    }
    let attachment = entry.get("attachment")?.as_object()?;
    (attachment.get("type").and_then(Value::as_str) == Some("queued_command")).then_some(attachment)
}

fn command_string<'a>(entry: &'a JsonObject, key: &str) -> Option<&'a str> {
    queued_command(entry)
        .and_then(|command| command.get(key))
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
}

fn delivery_key(id: &str) -> String {
    format!("delivery:{id}")
}

fn source_key(id: &str) -> String {
    format!("uuid:{id}")
}

fn entry_key(id: &str) -> String {
    format!("entry:{id}")
}

/// How many times each queued command was absorbed into a running turn (`Az`/`Oz`/`Tz`).
#[derive(Default)]
struct Delivered {
    counts: HashMap<String, usize>,
    copies: HashMap<String, String>,
    mixed: HashSet<String>,
    delivery_copies: IndexMap<String, Vec<String>>,
    delivery_copy_uuids: HashSet<String>,
}

impl Delivered {
    fn observe(&mut self, entry: &JsonObject) {
        if kind_of(entry) == Some("queue-operation")
            && str_field(entry, "operation") == Some("remove")
            && str_field(entry, "reason") == Some("absorbed_mid_turn")
            && (entry.get("deliveryId").is_some_and(Value::is_string)
                || entry.get("commandUuid").is_some_and(Value::is_string))
        {
            let key = match (str_field(entry, "deliveryId"), str_field(entry, "commandUuid")) {
                (Some(id), _) if !id.is_empty() => Some(delivery_key(id)),
                (_, Some(id)) => Some(source_key(id)),
                _ => None,
            };
            if let Some(key) = key {
                *self.counts.entry(key).or_insert(0) += 1;
            }
            return;
        }
        let (Some("attachment"), Some(uuid)) = (kind_of(entry), str_field(entry, "uuid")) else {
            return;
        };
        if let Some(delivery) = command_string(entry, "delivery_id") {
            if self.delivery_copy_uuids.insert(uuid.to_owned()) {
                self.delivery_copies
                    .entry(delivery_key(delivery))
                    .or_default()
                    .push(uuid.to_owned());
            }
            return;
        }
        let Some(source) = command_string(entry, "source_uuid") else {
            return;
        };
        let rendered = entry.get("attachment").map(Value::to_string).unwrap_or_default();
        match self.copies.get(source) {
            None => {
                self.copies.insert(source.to_owned(), rendered);
            }
            Some(seen) if *seen != rendered => {
                self.mixed.insert(source.to_owned());
            }
            Some(_) => {}
        }
    }

    fn finish(mut self) -> HashMap<String, usize> {
        for source in &self.mixed {
            self.counts.remove(&source_key(source));
        }
        for (key, uuids) in &self.delivery_copies {
            let wanted = self.counts.get(key).copied().unwrap_or(0);
            for uuid in &uuids[uuids.len().saturating_sub(wanted)..] {
                self.counts.insert(entry_key(uuid), 1);
            }
        }
        self.counts
    }
}

/// `YIe`: transcript entries and the delivered counts of one pass over the file.
pub(crate) fn parse_entries(content: &[u8]) -> (Vec<JsonObject>, HashMap<String, usize>) {
    let mut entries = Vec::new();
    let mut delivered = Delivered::default();
    for line in content.split(|byte| *byte == b'\n') {
        if line.iter().all(|byte| *byte <= 32) {
            continue;
        }
        let text = String::from_utf8_lossy(line);
        let Ok(Value::Object(parsed)) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let entry = fold_voice_call(parsed);
        if is_transcript_entry(&entry) {
            entries.push(entry.clone());
        }
        delivered.observe(&entry);
    }
    (entries, delivered.finish())
}

fn tool_ids(entry: &JsonObject, block_type: &str, key: &str) -> Vec<String> {
    entry
        .get("message")
        .and_then(Value::as_object)
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
        .map(|content| {
            content
                .iter()
                .filter_map(Value::as_object)
                .filter(|block| block.get("type").and_then(Value::as_str) == Some(block_type))
                .filter_map(|block| block.get(key).and_then(Value::as_str).map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn message_id(entry: &JsonObject) -> Option<&str> {
    if kind_of(entry) != Some("assistant") {
        return None;
    }
    entry.get("message")?.as_object()?.get("id")?.as_str()
}

/// `wz`: a user entry answering tool calls.
fn is_tool_result(entry: &JsonObject) -> bool {
    kind_of(entry) == Some("user")
        && parent_uuid(entry).is_some()
        && entry
            .get("message")
            .and_then(Value::as_object)
            .and_then(|message| message.get("content"))
            .and_then(Value::as_array)
            .is_some_and(|content| {
                content
                    .iter()
                    .any(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
            })
}

struct Linked {
    chain: Vec<JsonObject>,
    relinked: Entries,
    leaf: String,
}

fn relink_compactions(entries: &[JsonObject]) -> Result<Entries, MessagesError> {
    let mut map: Entries = IndexMap::new();
    for entry in entries {
        map.insert(uuid_of(entry).to_owned(), entry.clone());
    }
    let keys: Vec<String> = map.keys().cloned().collect();
    for key in &keys {
        let boundary = map[key].clone();
        if kind_of(&boundary) != Some("system") || str_field(&boundary, "subtype") != Some("compact_boundary") {
            continue;
        }
        let metadata = boundary.get("compactMetadata").and_then(Value::as_object);
        let preserved_messages = metadata.and_then(|metadata| metadata.get("preservedMessages"));
        let preserved_segment = metadata.and_then(|metadata| metadata.get("preservedSegment"));
        if truthy(preserved_messages) {
            let uuids = match preserved_messages
                .and_then(Value::as_object)
                .and_then(|object| object.get("uuids"))
            {
                None => return Err(MessagesError::UuidsLength("undefined")),
                Some(Value::Null) => return Err(MessagesError::UuidsLength("null")),
                Some(Value::Array(uuids)) => uuids,
                Some(Value::String(text)) if text.is_empty() => continue,
                Some(Value::Object(object)) if object.get("length").and_then(Value::as_i64) == Some(0) => continue,
                Some(_) => return Err(MessagesError::UuidsSome),
            };
            let object = preserved_messages
                .and_then(Value::as_object)
                .ok_or(MessagesError::UuidsSome)?;
            let names: Vec<Option<&str>> = uuids.iter().map(Value::as_str).collect();
            if names.is_empty() || names.iter().any(|name| name.is_none_or(|name| !map.contains_key(name))) {
                continue;
            }
            let names: Vec<&str> = names.into_iter().flatten().collect();
            let anchor = object.get("anchorUuid");
            let mut previous = anchor.cloned();
            for name in &names {
                if let Some(entry) = map.get_mut(*name) {
                    set_parent(entry, previous.as_ref());
                }
                previous = Some(Value::from(*name));
            }
            let (first, last) = (names[0], names[names.len() - 1]);
            for other in &keys {
                if other == first {
                    continue;
                }
                let Some(entry) = map.get_mut(other) else { continue };
                if entry.get("parentUuid") == anchor {
                    set_parent(entry, Some(&Value::from(last)));
                }
            }
        } else if truthy(preserved_segment) {
            let segment = preserved_segment.and_then(Value::as_object);
            let head = segment
                .and_then(|segment| segment.get("headUuid"))
                .and_then(Value::as_str);
            let anchor = segment.and_then(|segment| segment.get("anchorUuid"));
            if let Some(entry) = head.and_then(|head| map.get_mut(head)) {
                set_parent(entry, anchor);
            }
            let tail = segment.and_then(|segment| segment.get("tailUuid"));
            for other in &keys {
                if Some(other.as_str()) == head {
                    continue;
                }
                let Some(entry) = map.get_mut(other) else { continue };
                if entry.get("parentUuid") == anchor {
                    set_parent(entry, tail);
                }
            }
        }
    }
    Ok(map)
}

/// `vp`: the linked chain of visible entries ending at the selected leaf.
fn link(entries: &[JsonObject]) -> Result<Option<Linked>, MessagesError> {
    let map = relink_compactions(entries)?;
    let position: HashMap<&str, usize> = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| (uuid_of(entry), index))
        .collect();
    let rank = |entry: &JsonObject| position.get(uuid_of(entry)).map_or(-1, |index| *index as i64);
    let parents: HashSet<&str> = map.values().filter_map(parent_uuid).collect();
    let leaves: Vec<&JsonObject> = map.values().filter(|entry| !parents.contains(uuid_of(entry))).collect();
    let counts = |entry: &JsonObject| {
        !(flag(entry, "isSidechain")
            || flag(entry, "teamName")
            || kind_of(entry) == Some("progress")
            || (kind_of(entry) == Some("attachment")
                && entry
                    .get("attachment")
                    .and_then(Value::as_object)
                    .and_then(|attachment| attachment.get("type"))
                    .and_then(Value::as_str)
                    == Some("fork_briefing")))
    };
    let counted_parents: HashSet<&str> = map
        .values()
        .filter(|entry| counts(entry))
        .filter_map(parent_uuid)
        .collect();
    let mut tips: Vec<&JsonObject> = map
        .values()
        .filter(|entry| counts(entry) && !counted_parents.contains(uuid_of(entry)))
        .collect();
    tips.sort_by_key(|entry| std::cmp::Reverse(rank(entry)));

    let mut dead: HashSet<String> = HashSet::new();
    let mut terminal: Option<&JsonObject> = None;
    for tip in tips {
        let mut visited: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut current = Some(tip);
        while let Some(entry) = current {
            let uuid = uuid_of(entry);
            if dead.contains(uuid) || seen.contains(uuid) {
                break;
            }
            if is_conversation(entry) {
                terminal = Some(entry);
                break;
            }
            seen.insert(uuid.to_owned());
            visited.push(uuid.to_owned());
            current = parent_uuid(entry).and_then(|parent| map.get(parent));
        }
        if terminal.is_some() {
            break;
        }
        dead.extend(visited);
    }
    let leaf = match terminal {
        Some(entry) => entry,
        None => {
            let mut found: Vec<&JsonObject> = Vec::new();
            for start in &leaves {
                let mut seen: HashSet<&str> = HashSet::new();
                let mut current = Some(*start);
                while let Some(entry) = current {
                    if !seen.insert(uuid_of(entry)) {
                        break;
                    }
                    if is_conversation(entry) {
                        found.push(entry);
                        break;
                    }
                    current = parent_uuid(entry).and_then(|parent| map.get(parent));
                }
            }
            if found.is_empty() {
                return Ok(None);
            }
            let plain: Vec<&JsonObject> = found
                .iter()
                .copied()
                .filter(|entry| !flag(entry, "isSidechain") && !flag(entry, "teamName") && !flag(entry, "isMeta"))
                .collect();
            let pool = if plain.is_empty() { &found } else { &plain };
            let mut best = pool[0];
            for entry in pool.iter().skip(1) {
                if rank(entry) > rank(best) {
                    best = entry;
                }
            }
            best
        }
    };
    let mut chain: Vec<JsonObject> = Vec::new();
    let mut in_chain: HashSet<String> = HashSet::new();
    let mut current = map.get(uuid_of(leaf));
    while let Some(entry) = current {
        if !in_chain.insert(uuid_of(entry).to_owned()) {
            break;
        }
        chain.push(entry.clone());
        current = parent_uuid(entry).and_then(|parent| map.get(parent));
    }
    chain.reverse();
    let leaf_uuid = uuid_of(leaf).to_owned();
    let chain = regroup(&map, chain, &mut in_chain);
    Ok(Some(Linked {
        chain,
        relinked: map,
        leaf: leaf_uuid,
    }))
}

fn same_agent(a: &JsonObject, b: &JsonObject) -> bool {
    let sidechain = |entry: &JsonObject| {
        entry
            .get("isSidechain")
            .filter(|value| !value.is_null())
            .cloned()
            .unwrap_or(Value::Bool(false))
    };
    sidechain(a) == sidechain(b) && a.get("agentId") == b.get("agentId")
}

/// `String.prototype.localeCompare` with the default locale and options: root collation at tertiary strength.
pub(crate) fn locale_compare(left: &str, right: &str) -> std::cmp::Ordering {
    thread_local! {
        static COLLATOR: Option<icu_collator::CollatorBorrowed<'static>> = icu_collator::Collator::try_new(
            icu_collator::CollatorPreferences::default(),
            icu_collator::options::CollatorOptions::default(),
        )
        .ok();
    }
    COLLATOR.with(|collator| match collator {
        Some(collator) => collator.compare(left, right),
        None => left.cmp(right),
    })
}

fn timestamp_text(entry: &JsonObject) -> &str {
    str_field(entry, "timestamp").unwrap_or("")
}

/// `ZIe`: assistant fragments with the same message id and the tool results of their calls join the chain.
fn regroup(map: &Entries, chain: Vec<JsonObject>, in_chain: &mut HashSet<String>) -> Vec<JsonObject> {
    let assistants: Vec<&JsonObject> = chain
        .iter()
        .filter(|entry| kind_of(entry) == Some("assistant"))
        .collect();
    if assistants.is_empty() {
        return chain;
    }
    let mut chain_by_id: HashMap<&str, &JsonObject> = HashMap::new();
    for entry in &assistants {
        if let Some(id) = message_id(entry) {
            chain_by_id.insert(id, entry);
        }
    }
    let mut by_message: HashMap<&str, Vec<&JsonObject>> = HashMap::new();
    let mut call_owner: HashMap<String, Option<&JsonObject>> = HashMap::new();
    let mut results: Vec<&JsonObject> = Vec::new();
    for entry in map.values() {
        if let Some(id) = message_id(entry) {
            by_message.entry(id).or_default().push(entry);
            for call in tool_ids(entry, "tool_use", "id") {
                let next = match call_owner.get(&call) {
                    None => Some(entry),
                    Some(Some(owner)) if message_id(owner) == Some(id) => Some(entry),
                    Some(_) => None,
                };
                call_owner.insert(call, next);
            }
        } else if is_tool_result(entry) {
            results.push(entry);
        }
    }
    let mut attached: HashMap<String, Vec<&JsonObject>> = HashMap::new();
    let mut attached_seen: HashSet<String> = HashSet::new();
    let order: HashMap<&str, usize> = map
        .keys()
        .enumerate()
        .map(|(index, key)| (key.as_str(), index))
        .collect();
    for result in &results {
        let mut owners: Vec<String> = Vec::new();
        owners.push(parent_uuid(result).unwrap_or("").to_owned());
        if let Some(source) =
            str_field(result, "sourceToolAssistantUUID").filter(|source| Some(*source) != parent_uuid(result))
            && let Some(found) = map.get(source)
            && same_agent(result, found)
        {
            owners.push(uuid_of(found).to_owned());
        }
        for call in tool_ids(result, "tool_result", "tool_use_id") {
            if let Some(Some(found)) = call_owner.get(&call)
                && same_agent(result, found)
            {
                owners.push(uuid_of(found).to_owned());
            }
        }
        for owner in owners {
            if attached_seen.insert(format!("{owner}\n{}", uuid_of(result))) {
                attached.entry(owner).or_default().push(result);
            }
        }
    }
    let mut known_results: HashSet<String> = HashSet::new();
    for entry in &chain {
        known_results.extend(tool_ids(entry, "tool_result", "tool_use_id"));
    }
    let rank = |entry: &JsonObject| order.get(uuid_of(entry)).copied().unwrap_or(usize::MAX);
    let mut additions: HashMap<String, Vec<JsonObject>> = HashMap::new();
    let mut handled: HashSet<&str> = HashSet::new();
    for assistant in &assistants {
        let Some(id) = message_id(assistant) else { continue };
        if !handled.insert(id) {
            continue;
        }
        let group: Vec<&JsonObject> = by_message.get(id).cloned().unwrap_or_else(|| vec![assistant]);
        let group_ids: HashSet<&str> = group.iter().map(|entry| uuid_of(entry)).collect();
        let mut fragments: Vec<&JsonObject> = group
            .iter()
            .copied()
            .filter(|entry| !in_chain.contains(uuid_of(entry)))
            .collect();
        let mut with_parent: Vec<&JsonObject> = Vec::new();
        let mut without_parent: Vec<&JsonObject> = Vec::new();
        let mut seen: HashSet<&str> = HashSet::new();
        for member in &group {
            for result in attached.get(uuid_of(member)).into_iter().flatten() {
                let uuid = uuid_of(result);
                if in_chain.contains(uuid) || !seen.insert(uuid) {
                    continue;
                }
                if parent_uuid(result).is_some_and(|parent| group_ids.contains(parent)) {
                    with_parent.push(result);
                } else {
                    without_parent.push(result);
                }
            }
        }
        if !without_parent.is_empty() {
            let mut covered: HashSet<String> = known_results.clone();
            for result in &with_parent {
                covered.extend(tool_ids(result, "tool_result", "tool_use_id"));
            }
            without_parent.sort_by_key(|entry| rank(entry));
            for result in without_parent {
                let ids = tool_ids(result, "tool_result", "tool_use_id");
                let wanted = ids.iter().any(|call| {
                    !covered.contains(call)
                        && matches!(call_owner.get(call), Some(Some(owner)) if group_ids.contains(uuid_of(owner)))
                });
                covered.extend(ids);
                if wanted {
                    with_parent.push(result);
                }
            }
        }
        if fragments.is_empty() && with_parent.is_empty() {
            continue;
        }
        fragments.sort_by(|a, b| locale_compare(timestamp_text(a), timestamp_text(b)));
        with_parent.sort_by(|a, b| locale_compare(timestamp_text(a), timestamp_text(b)));
        let mut added: Vec<JsonObject> = Vec::new();
        for entry in fragments.into_iter().chain(with_parent) {
            in_chain.insert(uuid_of(entry).to_owned());
            added.push(entry.clone());
        }
        if let Some(anchor) = chain_by_id.get(id) {
            additions.insert(uuid_of(anchor).to_owned(), added);
        }
    }
    if additions.is_empty() {
        return chain;
    }
    let mut out = Vec::new();
    for entry in chain {
        let extra = additions.remove(uuid_of(&entry));
        out.push(entry);
        if let Some(extra) = extra {
            out.extend(extra);
        }
    }
    out
}

fn is_queued_attachment(entry: &JsonObject) -> bool {
    kind_of(entry) == Some("attachment")
        && entry
            .get("attachment")
            .and_then(Value::as_object)
            .and_then(|attachment| attachment.get("type"))
            .and_then(Value::as_str)
            == Some("queued_command")
}

/// `nLe`: queued command attachments that hang off the chosen leaf without belonging to the chain.
fn extra_attachments(entries: &[JsonObject], linked: &Linked) -> Vec<JsonObject> {
    let mut children: HashMap<&str, Vec<&JsonObject>> = HashMap::new();
    for entry in entries {
        if is_conversation(entry) {
            continue;
        }
        let own = parent_uuid(entry);
        let relinked = linked.relinked.get(uuid_of(entry)).and_then(parent_uuid);
        let mut keys: Vec<&str> = Vec::new();
        for key in [own, relinked].into_iter().flatten() {
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
        for key in keys {
            children.entry(key).or_default().push(entry);
        }
    }
    let mut seen: HashSet<String> = linked.chain.iter().map(|entry| uuid_of(entry).to_owned()).collect();
    let by_uuid: HashMap<&str, &JsonObject> = entries.iter().map(|entry| (uuid_of(entry), entry)).collect();
    let mut stranded: HashSet<&str> = HashSet::new();
    for entry in entries {
        if !is_conversation(entry)
            || flag(entry, "isSidechain")
            || flag(entry, "teamName")
            || seen.contains(uuid_of(entry))
        {
            continue;
        }
        let mut current = parent_uuid(entry).and_then(|parent| by_uuid.get(parent).copied());
        while let Some(parent) = current {
            if seen.contains(uuid_of(parent)) || stranded.contains(uuid_of(parent)) {
                break;
            }
            stranded.insert(uuid_of(parent));
            current = parent_uuid(parent).and_then(|next| by_uuid.get(next).copied());
        }
    }
    let Some(leaf) = linked.relinked.get(&linked.leaf) else {
        return Vec::new();
    };
    let mut found: Vec<JsonObject> = Vec::new();
    let mut stack: Vec<&JsonObject> = vec![leaf];
    while let Some(entry) = stack.pop() {
        if !std::ptr::eq(entry, leaf) {
            if seen.contains(uuid_of(entry)) {
                continue;
            }
            seen.insert(uuid_of(entry).to_owned());
            if is_queued_attachment(entry) && !stranded.contains(uuid_of(entry)) {
                found.push(entry.clone());
            }
        }
        let mut next: Vec<&JsonObject> = children.get(uuid_of(entry)).cloned().unwrap_or_default();
        if next.len() > 1 {
            next.sort_by(|a, b| locale_compare(timestamp_text(a), timestamp_text(b)));
        }
        for child in next.into_iter().rev() {
            if !seen.contains(uuid_of(child)) {
                stack.push(child);
            }
        }
    }
    found
}

/// `j$`: how a user entry relates to a local slash command.
fn local_command_part(entry: &JsonObject) -> Option<&'static str> {
    if entry.contains_key("promptSource") {
        return None;
    }
    let content = entry.get("message")?.as_object()?.get("content")?;
    let text = match content {
        Value::String(text) => text.as_str(),
        Value::Array(blocks) => blocks
            .iter()
            .rev()
            .find(|block| block.get("type").and_then(Value::as_str) == Some("text"))?
            .get("text")?
            .as_str()?,
        _ => return None,
    };
    [
        ("<command-name>", "record"),
        ("<local-command-stdout>", "output"),
        ("<local-command-stderr>", "output"),
        ("<local-command-caveat>", "caveat"),
    ]
    .into_iter()
    .find(|(prefix, _)| text.starts_with(prefix))
    .map(|(_, part)| part)
}

/// `XIe`: positions of the user entries that record a finished local command.
fn completed_local_commands(entries: &[JsonObject]) -> HashSet<usize> {
    let mut found = HashSet::new();
    for (start, entry) in entries.iter().enumerate() {
        if kind_of(entry) != Some("user") || !flag(entry, "isMeta") || local_command_part(entry) != Some("caveat") {
            continue;
        }
        let mut recorded = false;
        for (index, next) in entries.iter().enumerate().skip(start + 1) {
            if kind_of(next) == Some("assistant") {
                break;
            }
            if kind_of(next) != Some("user") || flag(next, "isMeta") {
                continue;
            }
            let part = local_command_part(next);
            if part == Some("record") && !recorded {
                recorded = true;
            } else if part != Some("output") || !recorded {
                break;
            }
            found.insert(index);
        }
    }
    found
}

/// `y0`: a user entry the CLI synthesizes when a turn is interrupted or a tool call is skipped.
fn is_interruption(entry: &JsonObject) -> bool {
    if kind_of(entry) != Some("user") {
        return false;
    }
    let starts = |text: &str| TS_REPLY_PREFIXES.iter().any(|prefix| text.starts_with(prefix));
    match entry
        .get("message")
        .and_then(Value::as_object)
        .and_then(|message| message.get("content"))
    {
        Some(Value::String(text)) => starts(text),
        Some(Value::Array(blocks)) => {
            !blocks.is_empty()
                && blocks.iter().all(|block| {
                    let text = match block.get("type").and_then(Value::as_str) {
                        Some("text") => block.get("text"),
                        Some("tool_result") if block.get("is_error") == Some(&Value::Bool(true)) => {
                            block.get("content")
                        }
                        _ => None,
                    };
                    text.and_then(Value::as_str).is_some_and(starts)
                })
        }
        _ => false,
    }
}

/// `JIe`: for each position, whether the next prompt-or-reply boundary after it is a reply.
fn reply_follows(entries: &[JsonObject], local: &HashSet<usize>) -> Vec<bool> {
    let mut flags = vec![false; entries.len()];
    let mut state: Option<bool> = None;
    for index in (0..entries.len()).rev() {
        let entry = &entries[index];
        flags[index] = state == Some(true);
        if kind_of(entry) == Some("assistant") || is_tool_result(entry) || is_interruption(entry) {
            state = Some(true);
        } else if kind_of(entry) == Some("user")
            && !flag(entry, "isMeta")
            && !flag(entry, "isCompactSummary")
            && !local.contains(&index)
        {
            state = Some(false);
        }
    }
    flags
}

/// `QIe`: whether each queued command copy was delivered to a running turn.
fn delivered_flags(entries: &[JsonObject], delivered: &HashMap<String, usize>) -> HashMap<usize, bool> {
    let mut flags = HashMap::new();
    if delivered.is_empty() {
        return flags;
    }
    let mut groups: IndexMap<String, Vec<usize>> = IndexMap::new();
    for (index, entry) in entries.iter().enumerate() {
        let key = match (
            command_string(entry, "delivery_id"),
            command_string(entry, "source_uuid"),
        ) {
            (Some(id), _) => delivery_key(id),
            (None, Some(id)) => source_key(id),
            _ => continue,
        };
        if delivered.contains_key(&key) {
            groups.entry(key).or_default().push(index);
        }
    }
    for (key, positions) in groups {
        if key.starts_with("delivery:") {
            for index in positions {
                flags.insert(index, delivered.contains_key(&entry_key(uuid_of(&entries[index]))));
            }
            continue;
        }
        let count = delivered.get(&key).copied().unwrap_or(0);
        let total = positions.len();
        for (slot, index) in positions.into_iter().enumerate() {
            flags.insert(index, slot + count >= total);
        }
    }
    flags
}

/// `QT`: origins whose meta messages stay visible.
fn origin_is_visible(origin: Option<&Value>) -> bool {
    let Some(object) = origin.and_then(Value::as_object) else {
        return false;
    };
    matches!(
        object.get("kind").and_then(Value::as_str),
        Some("channel" | "observer" | "observer-activity" | "slack-ping" | "voice-call" | "peer")
    )
}

/// `eLe`: a delivered queued command attachment becomes the user message it carried.
fn queued_to_user(entry: &JsonObject, wanted: bool, keep_meta: bool, uuids: &mut HashSet<String>) -> JsonObject {
    if !wanted || kind_of(entry) != Some("attachment") {
        return entry.clone();
    }
    let Some(attachment) = entry.get("attachment").and_then(Value::as_object) else {
        return entry.clone();
    };
    let prompt = attachment.get("prompt");
    let origin = attachment
        .get("origin")
        .filter(|origin| origin.get("kind").is_some_and(Value::is_string));
    if attachment.get("type").and_then(Value::as_str) != Some("queued_command")
        || (truthy(attachment.get("isMeta")) && !keep_meta && !origin_is_visible(origin))
        || !matches!(prompt, Some(Value::String(_) | Value::Array(_)))
    {
        return entry.clone();
    }
    let forwarded = attachment
        .get("forwardedIntent")
        .and_then(Value::as_object)
        .and_then(|intent| intent.get("lineage"))
        .and_then(Value::as_str)
        .is_some_and(|lineage| !lineage.is_empty());
    if forwarded {
        return entry.clone();
    }
    let uuid = attachment
        .get("source_uuid")
        .and_then(Value::as_str)
        .filter(|source| !source.is_empty())
        .unwrap_or_else(|| uuid_of(entry));
    if uuid != uuid_of(entry) && uuids.contains(uuid) {
        return entry.clone();
    }
    uuids.insert(uuid.to_owned());
    let mode = attachment.get("commandMode").and_then(Value::as_str);
    let mut out = Map::new();
    out.insert("type".to_owned(), Value::from("user"));
    out.insert("uuid".to_owned(), Value::from(uuid));
    for key in ["parentUuid", "sessionId", "timestamp"] {
        if let Some(value) = entry.get(key) {
            out.insert(key.to_owned(), value.clone());
        }
    }
    out.insert(
        "message".to_owned(),
        object([
            ("role", Value::from("user")),
            ("content", prompt.cloned().unwrap_or(Value::Null)),
        ]),
    );
    out.insert("isMeta".to_owned(), Value::Bool(truthy(attachment.get("isMeta"))));
    if let Some(origin) = command_origin(origin, mode, attachment.get("runId")) {
        out.insert("origin".to_owned(), origin);
    }
    out.insert("isQueuedCommand".to_owned(), Value::Bool(true));
    for key in ["isSidechain", "teamName"] {
        if let Some(value) = entry.get(key) {
            out.insert(key.to_owned(), value.clone());
        }
    }
    out
}

/// `iw`: marks completed local commands and turns delivered queued commands back into user messages.
fn annotate(
    entries: Vec<JsonObject>,
    trailing: &HashSet<String>,
    delivered: &HashMap<String, usize>,
    keep_meta: bool,
) -> Vec<JsonObject> {
    let local = completed_local_commands(&entries);
    let replies = reply_follows(&entries, &local);
    let flags = delivered_flags(&entries, delivered);
    let mut uuids: HashSet<String> = entries.iter().map(|entry| uuid_of(entry).to_owned()).collect();
    let mut out = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        if local.contains(&index) {
            let mut marked = entry.clone();
            marked.insert("isCompletedLocalCommand".to_owned(), Value::Bool(true));
            out.push(marked);
            continue;
        }
        let wanted = flags
            .get(&index)
            .copied()
            .unwrap_or(replies[index] || trailing.contains(uuid_of(entry)));
        out.push(queued_to_user(entry, wanted, keep_meta, &mut uuids));
    }
    out
}

/// `tLe`: which entries a reader returns.
fn is_visible(entry: &JsonObject, include_system: bool) -> bool {
    match kind_of(entry) {
        Some("user" | "assistant") => {}
        Some("system") if include_system => {}
        _ => return false,
    }
    if flag(entry, "isMeta") && !origin_is_visible(entry.get("origin").filter(|origin| !origin.is_null())) {
        return false;
    }
    !flag(entry, "isSidechain") && !flag(entry, "teamName")
}

/// `aw`: the public message of a transcript entry.
fn to_message(entry: &JsonObject, parent_tool_use_id: Option<&str>, parent_agent_id: Option<&str>) -> SessionMessage {
    let is_true = |key: &str| entry.get(key) == Some(&Value::Bool(true));
    let blocks: Vec<&Value> = entry
        .get("message")
        .and_then(Value::as_object)
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
        .map(|content| {
            content
                .iter()
                .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
                .collect()
        })
        .unwrap_or_default();
    let denial = entry
        .get("toolDenialKind")
        .and_then(Value::as_str)
        .filter(|kind| TS_DENIAL_KINDS.contains(kind));
    let tool_result_meta = match (denial, blocks.as_slice()) {
        (Some(kind), [block]) => block.get("tool_use_id").and_then(Value::as_str).map(|id| {
            Value::Array(vec![object([
                ("id", Value::from(id)),
                ("non_execution_kind", Value::from(kind)),
            ])])
        }),
        _ => None,
    };
    SessionMessage {
        kind: match kind_of(entry) {
            Some("user") => SessionMessageKind::User,
            Some("system") => SessionMessageKind::System,
            _ => SessionMessageKind::Assistant,
        },
        uuid: uuid_of(entry).to_owned(),
        session_id: str_field(entry, "sessionId").unwrap_or("").to_owned(),
        message: entry.get("message").cloned().unwrap_or(Value::Null),
        parent_tool_use_id: parent_tool_use_id.map(str::to_owned),
        parent_agent_id: parent_agent_id.map(str::to_owned),
        extras: MessageExtras {
            timestamp: entry.get("timestamp").cloned(),
            origin: entry
                .get("origin")
                .filter(|origin| !origin.is_null())
                .map(normalized_origin)
                .filter(|origin| truthy(Some(origin))),
            is_meta: is_true("isMeta") || is_true("isCompactSummary") || is_true("isVisibleInTranscriptOnly"),
            is_compact_summary: is_true("isCompactSummary"),
            is_queued_command: is_true("isQueuedCommand"),
            is_completed_local_command: is_true("isCompletedLocalCommand"),
            interrupted_by_shutdown: is_true("interruptedByShutdown"),
            tool_denial_unanswered: (str_field(entry, "toolDenialUnanswered") == Some("stream-closed"))
                .then(|| "stream-closed".to_owned()),
            tool_result_meta,
        },
    }
}

/// Where the TypeScript pipeline throws while rebuilding a conversation. The messages are the V8 `TypeError` texts of
/// the minified upstream bundle.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MessagesError {
    /// `compactMetadata.preservedMessages` has no usable `uuids` list.
    #[error("Cannot read properties of {0} (reading 'length')")]
    UuidsLength(&'static str),
    /// `compactMetadata.preservedMessages.uuids` is not an array.
    #[error("E.uuids.some is not a function")]
    UuidsSome,
}

/// `xz` without paging: the visible messages of a transcript.
pub(crate) fn messages_from_entries(
    entries: &[JsonObject],
    delivered: &HashMap<String, usize>,
    include_system: bool,
) -> Result<Vec<SessionMessage>, MessagesError> {
    let Some(linked) = link(entries)? else {
        return Ok(Vec::new());
    };
    let extra = extra_attachments(entries, &linked);
    let trailing: HashSet<String> = extra.iter().map(|entry| uuid_of(entry).to_owned()).collect();
    let mut combined = linked.chain.clone();
    combined.extend(extra);
    Ok(annotate(combined, &trailing, delivered, false)
        .iter()
        .filter(|entry| is_visible(entry, include_system))
        .map(|entry| to_message(entry, None, None))
        .collect())
}

/// `kz` after the file is read: parse, rebuild the conversation and page it.
pub(crate) fn session_messages(
    content: &[u8],
    include_system: bool,
    page: MessagePage,
) -> Result<Vec<SessionMessage>, MessagesError> {
    let (entries, delivered) = parse_entries(content);
    Ok(page.apply(messages_from_entries(&entries, &delivered, include_system)?))
}

/// `Aw`: a subagent transcript, whose chain is the last user or assistant entry walked back through `parentUuid`.
pub(crate) fn subagent_messages(
    content: &[u8],
    page: MessagePage,
    tool_use_id: Option<&str>,
    parent_agent_id: Option<&str>,
) -> Vec<SessionMessage> {
    if content.is_empty() {
        return Vec::new();
    }
    let mut entries: Vec<JsonObject> = Vec::new();
    for line in content.split(|byte| *byte == b'\n') {
        if line.iter().all(|byte| *byte <= 32) {
            continue;
        }
        let Ok(Value::Object(parsed)) = serde_json::from_str::<Value>(&String::from_utf8_lossy(line)) else {
            continue;
        };
        let entry = fold_voice_call(parsed);
        if matches!(kind_of(&entry), Some("user" | "assistant" | "attachment"))
            && entry.get("uuid").is_some_and(Value::is_string)
        {
            entries.push(entry);
        }
    }
    let by_uuid: HashMap<&str, &JsonObject> = entries.iter().map(|entry| (uuid_of(entry), entry)).collect();
    let Some(leaf) = entries.iter().rev().find(|entry| is_conversation(entry)) else {
        return Vec::new();
    };
    let mut chain: Vec<JsonObject> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    let mut current = Some(leaf);
    while let Some(entry) = current {
        if !seen.insert(uuid_of(entry)) {
            break;
        }
        chain.push(entry.clone());
        current = parent_uuid(entry).and_then(|parent| by_uuid.get(parent).copied());
    }
    chain.reverse();
    let tool_use_id = tool_use_id.filter(|id| valid_run_id(Some(&Value::from(*id))).is_some());
    let parent_agent_id = parent_agent_id.filter(|id| {
        (1..=128).contains(&id.len())
            && id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    });
    let messages = annotate(chain, &HashSet::new(), &HashMap::new(), true)
        .iter()
        .filter(|entry| is_conversation(entry))
        .map(|entry| to_message(entry, tool_use_id, parent_agent_id))
        .collect();
    page.apply(messages)
}

#[cfg(test)]
#[path = "session_messages_ts_tests.rs"]
mod tests;
