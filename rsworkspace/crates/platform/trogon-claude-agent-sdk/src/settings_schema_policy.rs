//! The policy-tolerant schema parse of a managed settings document (`cP` upstream).
//!
//! A managed document is not rejected for a bad field. Each carried key is read on its own: a lock that
//! cannot be read becomes its restrictive value, a list loses the entries that cannot be read, and a grant
//! sitting beside a restriction that could not be read is withheld. Only the values are reproduced here,
//! with the one diagnostic the resolver acts on: whether every key the document carries holds only a value
//! put in place of what was written.
//!
//! This module lives under `settings_schema` because it walks the private schema nodes.

use std::{
    cell::Cell,
    collections::{BTreeSet, HashMap, HashSet},
    sync::Arc,
};

use serde_json::{Map, Value};

use super::{
    Check, Mode, Node, SettingsSchema, all_pass,
    policy_lists::{self, ListKind},
};
use crate::{
    constants::{POLICY_META_KEYS, POLICY_NEVER_SUBSTITUTED, POLICY_SKELETON_EXCLUDED, POLICY_UNSYNTHESIZED_BLOCKS},
    settings_policy_content::{has_policy_content, is_empty_tree},
};

#[path = "settings_schema_policy_credentials.rs"]
mod credentials;

/// One block whose grants are withheld when one of its restrictions cannot be read (`J5e`).
struct Block {
    name: String,
    restrictions: Vec<String>,
    grants: Vec<String>,
    withhold_on_entry_drop: bool,
}

/// The reader of the one enum-like grant that has a floor (`WS`, the default permission mode).
struct DefaultMode {
    path: String,
    modes: Vec<String>,
    aliases: Vec<(String, String)>,
    inert: Vec<String>,
    floor: String,
}

impl DefaultMode {
    /// `read`: the mode the value names, after the alias spelling is resolved.
    fn read(&self, value: &Value) -> Option<String> {
        let text = value.as_str()?;
        let canonical = self
            .aliases
            .iter()
            .find(|(alias, _)| alias == text)
            .map_or(text, |(_, canonical)| canonical.as_str());
        self.modes.iter().find(|mode| mode.as_str() == canonical).cloned()
    }
}

pub(super) struct PolicySpec {
    /// `Go()`: every restrictive reading, by path.
    restrictive: Vec<(Vec<String>, Value)>,
    /// `Pu()`: the first scalar of each restrictive reading, by dotted path, for nested paths.
    nested: HashMap<String, Value>,
    blocks: Vec<Block>,
    default_mode: DefaultMode,
}

fn malformed(what: &str) -> String {
    format!("read.policy.{what}")
}

fn string_list(value: &Value, what: &str) -> Result<Vec<String>, String> {
    value
        .as_array()
        .ok_or_else(|| malformed(what))?
        .iter()
        .map(|item| item.as_str().map(str::to_owned).ok_or_else(|| malformed(what)))
        .collect()
}

impl PolicySpec {
    pub(super) fn from_json(read: &Value) -> Result<Self, String> {
        let policy = read.get("policy").ok_or_else(|| malformed("policy"))?;
        let restrictive = policy
            .get("restrictive")
            .and_then(Value::as_array)
            .ok_or_else(|| malformed("restrictive"))?
            .iter()
            .map(|entry| {
                let path = string_list(entry.get("path")?, "path").ok()?;
                Some((path, entry.get("restrictive")?.clone()))
            })
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| malformed("restrictive"))?;
        let nested = restrictive
            .iter()
            .filter(|(path, _)| path.len() > 1)
            .filter_map(|(path, value)| {
                let first = match value {
                    Value::Array(items) => items.first()?.clone(),
                    other => other.clone(),
                };
                Some((path.join("."), first))
            })
            .collect();
        let blocks = read
            .get("permissionBlocks")
            .and_then(Value::as_array)
            .ok_or_else(|| malformed("permissionBlocks"))?
            .iter()
            .map(|block| -> Result<Block, String> {
                Ok(Block {
                    name: block
                        .get("name")
                        .and_then(Value::as_str)
                        .ok_or_else(|| malformed("block name"))?
                        .to_owned(),
                    restrictions: string_list(block.get("restrictions").unwrap_or(&Value::Null), "restrictions")?,
                    grants: string_list(block.get("grants").unwrap_or(&Value::Null), "grants")?,
                    withhold_on_entry_drop: block
                        .get("withholdOnEntryDrop")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let mode = policy.get("defaultMode").ok_or_else(|| malformed("defaultMode"))?;
        let default_mode = DefaultMode {
            path: mode
                .get("path")
                .and_then(Value::as_str)
                .ok_or_else(|| malformed("defaultMode path"))?
                .to_owned(),
            modes: string_list(mode.get("modes").unwrap_or(&Value::Null), "modes")?,
            aliases: mode
                .get("aliases")
                .and_then(Value::as_object)
                .ok_or_else(|| malformed("aliases"))?
                .iter()
                .map(|(alias, canonical)| {
                    canonical
                        .as_str()
                        .map(|canonical| (alias.clone(), canonical.to_owned()))
                        .ok_or_else(|| malformed("aliases"))
                })
                .collect::<Result<_, _>>()?,
            inert: string_list(mode.get("inert").unwrap_or(&Value::Null), "inert")?,
            floor: mode
                .get("floor")
                .and_then(Value::as_str)
                .ok_or_else(|| malformed("floor"))?
                .to_owned(),
        };
        Ok(Self {
            restrictive,
            nested,
            blocks,
            default_mode,
        })
    }
}

/// What one block parse could not read, by key (`i` and `a` upstream), and which members hold a value put in
/// place of what was written (`s` upstream) or a block made up from restrictive values alone.
#[derive(Default)]
struct Findings {
    unreadable: HashSet<String>,
    trimmed: HashSet<String>,
    substituted: HashSet<String>,
    synthesized: HashSet<String>,
}

/// The policy read of one document (`cP(...).safeParse(document)` and its closing transform).
pub(crate) struct PolicyParse {
    pub(crate) settings: Map<String, Value>,
    /// Every key that carries content holds only a value the reader put in place of what was written (the
    /// `onlySubstitutes` diagnostic upstream).
    pub(crate) only_substitutes: bool,
    /// The default mode holds the value put in place of one that could not be read (an error at its path marked
    /// `substituted` upstream).
    pub(crate) floored: bool,
}

/// What every block of one document parse shares: where the document came from, and whether a block put a value in
/// place of a default mode it could not read.
struct Reading<'a> {
    file: Option<&'a str>,
    floored: Cell<bool>,
}

/// A value read as written, never a stand-in.
fn as_written(value: Option<Value>) -> Option<(Value, bool)> {
    value.map(|value| (value, false))
}

/// The parts of an object schema a block read walks.
struct Shape<'a> {
    mode: &'a Mode,
    members: &'a [(String, Arc<Node>)],
    checks: &'a [Check],
}

/// `aP`: the object schema behind optional and pipe wrappers.
fn object_of(node: &Node) -> Option<Shape<'_>> {
    match node {
        Node::Optional(inner) | Node::Lazy(inner) => object_of(inner),
        Node::Pipe { output, .. } => object_of(output),
        Node::Object { mode, shape, checks } => Some(Shape {
            mode,
            members: shape,
            checks,
        }),
        _ => None,
    }
}

/// `vu`: the strings "true" and "false" read as booleans.
fn read_boolean_text(value: Option<&Value>) -> Option<Value> {
    match value {
        Some(Value::String(text)) if text == "true" => Some(Value::Bool(true)),
        Some(Value::String(text)) if text == "false" => Some(Value::Bool(false)),
        other => other.cloned(),
    }
}

/// `aJ`: false, spelled either way.
fn is_false(value: &Value) -> bool {
    matches!(value, Value::Bool(false)) || value.as_str() == Some("false")
}

/// `qk`: the preparation of a locked value before its own schema reads it.
fn prepare(restrictive: Option<&Value>, input: Option<&Value>) -> Option<Value> {
    match restrictive {
        Some(Value::Bool(_)) => read_boolean_text(input),
        Some(Value::String(text)) if text == "disable" => input.filter(|value| !is_false(value)).cloned(),
        _ => input.cloned(),
    }
}

/// The indices of the entries the schema reports as invalid for an array input (issue paths that start with
/// an index).
fn failing_entries(node: &Node, value: &Value) -> BTreeSet<usize> {
    match node {
        Node::Optional(inner) | Node::Lazy(inner) => failing_entries(inner, value),
        Node::Array { element, .. } => match value {
            Value::Array(items) => items
                .iter()
                .enumerate()
                .filter(|(_, item)| element.validate(Some(item)).is_err())
                .map(|(index, _)| index)
                .collect(),
            _ => BTreeSet::new(),
        },
        Node::Pipe { input, output } => match input.validate(Some(value)) {
            Err(()) => failing_entries(input, value),
            Ok(Some(middle)) => failing_entries(output, &middle),
            Ok(None) => BTreeSet::new(),
        },
        _ => BTreeSet::new(),
    }
}

impl SettingsSchema {
    /// `cP(...).safeParse(document)`: the carried keys read one by one, unknown keys passed through. It
    /// never fails. `file` is where the document came from.
    ///
    /// A key whose reader yields `undefined` is never inserted, which is what the closing transform does when
    /// it deletes undefined keys. The `wrongDocumentIssues` check before it only flags a key the parsed value
    /// holds but the document `cP` was built from did not; both callers (`j7e` and `lE`) parse the very document
    /// they built the schema from, so the check never adds an issue and has no counterpart here.
    pub(crate) fn parse_policy(&self, document: &Map<String, Value>, file: Option<&str>) -> PolicyParse {
        let mut settings = Map::new();
        let mut stand_ins = HashSet::new();
        let reading = Reading {
            file,
            floored: Cell::new(false),
        };
        for key in &self.keys {
            let Some(value) = document.get(key) else {
                continue;
            };
            if let Some((output, stand_in)) = self.policy_value(key, value, &reading) {
                if stand_in {
                    stand_ins.insert(key.clone());
                }
                settings.insert(key.clone(), output);
            }
        }
        for (key, value) in document {
            if !self.fields.contains_key(key) {
                settings.insert(key.clone(), value.clone());
            }
        }
        let mut carried = settings
            .iter()
            .filter(|(key, value)| !POLICY_META_KEYS.contains(&key.as_str()) && !is_empty_tree(value))
            .peekable();
        let only_substitutes = carried.peek().is_some() && carried.all(|(key, _)| stand_ins.contains(key));
        PolicyParse {
            settings,
            only_substitutes,
            floored: reading.floored.get(),
        }
    }

    /// One carried key, with whether its value stands in for what was written (`l` and the synthesized set
    /// upstream).
    fn policy_value(&self, key: &str, value: &Value, reading: &Reading<'_>) -> Option<(Value, bool)> {
        let field = self.fields.get(key)?;
        let generic = || as_written(field.validate(Some(value)).unwrap_or(None));
        if self.is_lock_block(key) {
            let strict = (key != "sandbox").then_some(&**field);
            return match object_of(field) {
                Some(shape) => self.read_block(key, &shape, value, strict, reading),
                None => generic(),
            };
        }
        match key {
            "wslInheritsWindowsSettings" => {
                if value.is_null() {
                    return None;
                }
                as_written(field.validate(read_boolean_text(Some(value)).as_ref()).unwrap_or(None))
            }
            "strictPluginOnlyCustomization" => {
                if value.is_null() {
                    return None;
                }
                match unwrap_catch_optional(field).validate(Some(value)) {
                    Ok(output) => as_written(output),
                    Err(()) => Some((Value::Bool(true), true)),
                }
            }
            "policyHelpers" => as_written(read_policy_helpers(field, value)),
            "managedMcpServers" => as_written(read_managed_mcp_servers(field, value)),
            "strictKnownMarketplaces" => as_written(policy_lists::marketplaces(field, ListKind::Allow, value)),
            "blockedMarketplaces" => as_written(policy_lists::marketplaces(field, ListKind::Block, value)),
            "allowedMcpServers" => as_written(policy_lists::server_list(field, ListKind::Allow, value)),
            "deniedMcpServers" => as_written(policy_lists::server_list(field, ListKind::Block, value)),
            "enabledPlugins" => as_written(policy_lists::enabled_plugins(value)),
            "availableModels" => as_written(policy_lists::models(ListKind::Allow, value)),
            "deniedModels" => as_written(policy_lists::models(ListKind::Block, value)),
            "allowedHttpHookUrls" | "httpHookAllowedEnvVars" | "allowedChannelPlugins" | "allowedProviders" => {
                Some((policy_lists::allowlist(key, value), false))
            }
            "forceLoginOrgUUID" => as_written(match field.validate(Some(value)) {
                Ok(output) => output,
                Err(()) => Some(Value::Array(Vec::new())),
            }),
            _ => match self.top_level_restrictive(key) {
                Some(restrictive) => {
                    if value.is_null() {
                        return None;
                    }
                    match field.validate(prepare(Some(restrictive), Some(value)).as_ref()) {
                        Ok(output) => as_written(output),
                        Err(()) => Some((restrictive.clone(), true)),
                    }
                }
                None => generic(),
            },
        }
    }

    /// `rP`: the top-level locks read with their own restrictive fallback.
    fn top_level_restrictive(&self, key: &str) -> Option<&Value> {
        if key == "strictPluginOnlyCustomization" || key == "disableAllHooks" {
            return None;
        }
        let readable = self
            .fields
            .get(key)
            .is_some_and(|node| !matches!(**node, Node::Catch { .. }));
        self.policy
            .restrictive
            .iter()
            .find(|(path, restrictive)| {
                path.len() == 1 && path[0] == key && (restrictive.is_boolean() || restrictive.is_string())
            })
            .filter(|_| readable)
            .map(|(_, restrictive)| restrictive)
    }

    /// `HS`: some lock lives below the path.
    fn has_locked_children(&self, path: &str) -> bool {
        let prefix = format!("{path}.");
        self.policy.nested.keys().any(|name| name.starts_with(&prefix))
    }

    /// `VS`: a block whose locks are read one by one (the credentials block has its own reader).
    fn is_lock_block(&self, path: &str) -> bool {
        path != "sandbox.credentials" && !path.starts_with("sandbox.credentials.") && self.has_locked_children(path)
    }

    /// `Yk`: reads one block field by field, with whether the block is made up only of values put in place of
    /// what was written. `None` is an unset block.
    fn read_block(
        &self,
        path: &str,
        shape: &Shape<'_>,
        input: &Value,
        strict: Option<&Node>,
        reading: &Reading<'_>,
    ) -> Option<(Value, bool)> {
        if input.is_null() {
            return None;
        }
        let converted = match (input.is_object(), strict) {
            (false, Some(strict)) => match strict.validate(Some(input)) {
                Ok(Some(data)) if data.is_object() => Some(data),
                _ => None,
            },
            _ => None,
        };
        let block = converted.as_ref().unwrap_or(input);
        let mut findings = Findings::default();
        let parsed = block.as_object().and_then(|entries| {
            let present: Map<String, Value> = entries
                .iter()
                .filter(|(_, value)| !value.is_null())
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect();
            self.parse_block(path, shape, &present, &mut findings, reading)
        });
        let Some(mut parsed) = parsed else {
            let skeleton = self.skeleton(path, shape);
            return if skeleton.is_empty() {
                None
            } else {
                Some((Value::Object(skeleton), synthesizes(path)))
            };
        };
        self.withhold_grants(path, &mut parsed, &mut findings);
        let at_default_mode = |name: &String| format!("{path}.{name}") == self.policy.default_mode.path;
        if findings.substituted.iter().any(at_default_mode) {
            reading.floored.set(true);
        }
        if parsed.is_empty() {
            let carries_known_key = block.as_object().is_some_and(|entries| {
                entries
                    .keys()
                    .any(|name| shape.members.iter().any(|(known, _)| known == name))
            });
            return if carries_known_key {
                None
            } else {
                Some((Value::Object(parsed), false))
            };
        }
        let synthesized = synthesizes(path)
            && parsed.iter().all(|(name, value)| {
                findings.substituted.contains(name) || findings.synthesized.contains(name) || is_empty_tree(value)
            });
        Some((Value::Object(parsed), synthesized))
    }

    /// The block schema extended with the readers of its members (`t.safeExtend(l)` parsed).
    fn parse_block(
        &self,
        path: &str,
        shape: &Shape<'_>,
        entries: &Map<String, Value>,
        findings: &mut Findings,
        reading: &Reading<'_>,
    ) -> Option<Map<String, Value>> {
        let mut output = Map::new();
        for (name, member) in shape.members {
            let child = format!("{path}.{name}");
            let present = entries.get(name);
            let read = match object_of(member) {
                _ if child == "sandbox.credentials" => {
                    present.and_then(|value| credentials::read(member, reading.file, value))
                }
                Some(inner) if self.is_lock_block(&child) => {
                    present.and_then(|value| self.read_block(&child, &inner, value, Some(&**member), reading))
                }
                _ => as_written(self.read_member(&child, member, present, findings, name)),
            };
            if let Some((value, synthesized)) = read {
                if synthesized {
                    findings.synthesized.insert(name.clone());
                }
                output.insert(name.clone(), value);
            }
        }
        let known = |name: &String| shape.members.iter().any(|(key, _)| key == name);
        match shape.mode {
            Mode::Strip => {}
            Mode::Strict if entries.keys().any(|name| !known(name)) => return None,
            Mode::Strict => {}
            Mode::Passthrough => {
                for (name, value) in entries {
                    if !known(name) {
                        output.insert(name.clone(), value.clone());
                    }
                }
            }
        }
        let output = Value::Object(output);
        if !all_pass(shape.checks, &output) {
            return None;
        }
        output.as_object().cloned()
    }

    /// `X5e`: one member of a block. A member that cannot be read becomes its restrictive value, the floor
    /// of its reader, the entries of its list that can be read, or nothing, in that order.
    fn read_member(
        &self,
        path: &str,
        member: &Node,
        input: Option<&Value>,
        findings: &mut Findings,
        name: &str,
    ) -> Option<Value> {
        let restrictive = self.policy.nested.get(path);
        let schema = match member {
            Node::Catch { inner, .. } => &**inner,
            other => other,
        };
        let prepared = prepare(restrictive, input);
        if let Ok(output) = schema.validate(prepared.as_ref()) {
            return output;
        }
        if let Some(value) = restrictive.filter(|_| !POLICY_NEVER_SUBSTITUTED.contains(&path)) {
            findings.substituted.insert(name.to_owned());
            return Some(value.clone());
        }
        if path == self.policy.default_mode.path {
            findings.substituted.insert(name.to_owned());
            return Some(Value::String(self.policy.default_mode.floor.clone()));
        }
        if schema.validate(Some(&Value::Array(Vec::new()))).is_ok()
            && let Some(Value::Array(items)) = input
        {
            let failing = failing_entries(schema, &Value::Array(items.clone()));
            let kept: Vec<Value> = items
                .iter()
                .enumerate()
                .filter(|(index, _)| !failing.contains(index))
                .map(|(_, item)| item.clone())
                .collect();
            if !failing.is_empty()
                && failing.len() < items.len()
                && let Ok(data) = schema.validate(Some(&Value::Array(kept)))
            {
                findings.trimmed.insert(name.to_owned());
                return data;
            }
        }
        findings.unreadable.insert(name.to_owned());
        None
    }

    /// `iJ`: the block as its restrictive values alone.
    fn skeleton(&self, path: &str, shape: &Shape<'_>) -> Map<String, Value> {
        let mut output = Map::new();
        for (name, member) in shape.members {
            let child = format!("{path}.{name}");
            if POLICY_SKELETON_EXCLUDED.contains(&child.as_str()) || POLICY_NEVER_SUBSTITUTED.contains(&child.as_str())
            {
                continue;
            }
            if let Some(value) = self.policy.nested.get(&child) {
                output.insert(name.clone(), value.clone());
            } else if let Some(inner) = object_of(member)
                && self.has_locked_children(&child)
            {
                output.insert(name.clone(), Value::Object(self.skeleton(&child, &inner)));
            }
        }
        output
    }

    /// `iP`: grants beside a restriction that could not be read are withheld.
    fn withhold_grants(&self, path: &str, block: &mut Map<String, Value>, findings: &mut Findings) {
        let Some(rule) = self.policy.blocks.iter().find(|rule| rule.name == path) else {
            return;
        };
        let restricted = rule.restrictions.iter().any(|name| {
            findings.unreadable.contains(name) || (rule.withhold_on_entry_drop && findings.trimmed.contains(name))
        });
        if !restricted {
            return;
        }
        let mode = &self.policy.default_mode;
        for grant in &rule.grants {
            let is_mode = format!("{path}.{grant}") == mode.path;
            let current = block.get(grant);
            if is_mode {
                let Some(read) = current.and_then(|value| mode.read(value)) else {
                    continue;
                };
                if mode.inert.contains(&read) {
                    continue;
                }
                block.insert(grant.clone(), Value::String(mode.floor.clone()));
                findings.substituted.insert(grant.clone());
            } else if current.is_some() {
                block.shift_remove(grant);
            }
        }
    }

    /// The `WS` loop of `UP` upstream for the default mode, the one grant with a floor. A document whose default mode
    /// stands in for one it could not read leaves that stand-in out of `settings` when an earlier document of the folder
    /// wrote an inert mode into `merged` and no floor is held, and otherwise holds the floor exactly when its mode is a
    /// stand-in. Returns whether the folder holds the floor after the document.
    pub(crate) fn hold_default_mode_floor(
        &self,
        merged: &Map<String, Value>,
        settings: &mut Map<String, Value>,
        substituted: bool,
        floored: bool,
    ) -> bool {
        let mode = &self.policy.default_mode;
        let path: Vec<&str> = mode.path.split('.').collect();
        if value_at(settings, &path).is_none() {
            return floored;
        }
        let earlier = value_at(merged, &path).and_then(|current| mode.read(current));
        if substituted && !floored && earlier.is_some_and(|current| mode.inert.contains(&current)) {
            let _ = unset_path(settings, &path);
            return floored;
        }
        substituted
    }

    /// `NX`: whether the raw document holds anything that is policy and not merely an unset or a
    /// restriction spelled as nothing, or the parsed settings hold a key beyond the meta keys.
    pub(crate) fn policy_has_content(&self, raw: &Map<String, Value>, settings: &Map<String, Value>) -> bool {
        raw.iter().any(|(name, value)| {
            !value.is_null() && !self.reads_as_nothing(name, value) && !POLICY_META_KEYS.contains(&name.as_str())
        }) || has_policy_content(settings)
    }

    /// `SX`.
    fn reads_as_nothing(&self, name: &str, value: &Value) -> bool {
        let cleared = &self.read.null_cleared;
        let clears = cleared.iter().any(|key| key == name)
            || self
                .read
                .aliases
                .iter()
                .any(|(alias, canonical)| alias == name && cleared.contains(canonical));
        if clears {
            self.is_unset(name, value)
        } else {
            self.is_lock_block(name) && self.is_empty_lock_block(name, value)
        }
    }

    /// `lP`: null, or false where the only value of the key is "disable".
    fn is_unset(&self, name: &str, value: &Value) -> bool {
        if value.is_null() {
            return true;
        }
        let restrictive = if name.contains('.') {
            self.policy.nested.get(name)
        } else {
            self.top_level_restrictive(name)
        };
        restrictive.and_then(Value::as_str) == Some("disable") && is_false(value)
    }

    /// `EX`.
    fn is_empty_lock_block(&self, name: &str, value: &Value) -> bool {
        let Value::Object(entries) = value else {
            return false;
        };
        !entries.is_empty()
            && entries.iter().all(|(key, entry)| {
                let child = format!("{name}.{key}");
                self.is_unset(&child, entry) || (self.is_lock_block(&child) && self.is_empty_lock_block(&child, entry))
            })
    }
}

/// The member of an object schema by name.
fn member_of<'a>(shape: &Shape<'a>, name: &str) -> Option<&'a Node> {
    shape
        .members
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, node)| &**node)
}

/// `cP`'s own reader of the per-OS policy helper map. An entry that cannot be read is dropped on its own instead of
/// the whole map, and a map that is not an object reads as an empty map.
fn read_policy_helpers(field: &Node, value: &Value) -> Option<Value> {
    let shape = object_of(unwrap_catch_optional(field))?;
    match value {
        Value::Null => None,
        Value::Object(entries) => Some(Value::Object(
            shape
                .members
                .iter()
                .filter_map(|(name, member)| Some((name.clone(), read_policy_helper(member, entries.get(name)?)?)))
                .collect(),
        )),
        _ => Some(Value::Object(Map::new())),
    }
}

/// One entry of the policy helper map (`g` inside `cP`). A static payload that cannot be read is left out of the
/// entry. An OS entry that still cannot be read keeps its static payload alone when that payload and its output
/// behavior can be read, and is dropped otherwise; the `default` entry is a payload itself and has no entry fields.
fn read_policy_helper(member: &Node, input: &Value) -> Option<Value> {
    let entry = object_of(member);
    let payload = entry.as_ref().and_then(|shape| member_of(shape, "defaultSettings"));
    let mut readable = input.clone();
    if let (Some(payload), Value::Object(fields)) = (payload, &mut readable)
        && fields
            .get("defaultSettings")
            .is_some_and(|settings| payload.validate(Some(settings)).is_err())
    {
        fields.shift_remove("defaultSettings");
    }
    match member.validate(Some(&readable)) {
        Ok(output) => output,
        Err(()) => entry.and_then(|shape| kept_payload(&shape, input)),
    }
}

/// The static payload an OS entry that cannot be read keeps, with its output behavior when it names one.
fn kept_payload(shape: &Shape<'_>, input: &Value) -> Option<Value> {
    let fields = input.as_object()?;
    let settings = fields.get("defaultSettings").filter(|settings| !settings.is_null())?;
    let mut kept = Map::new();
    kept.insert(
        "defaultSettings".to_owned(),
        member_of(shape, "defaultSettings")?.validate(Some(settings)).ok()??,
    );
    if let Some(behavior) = fields.get("outputBehavior").filter(|behavior| !behavior.is_null()) {
        kept.insert(
            "outputBehavior".to_owned(),
            member_of(shape, "outputBehavior")?.validate(Some(behavior)).ok()??,
        );
    }
    Some(Value::Object(kept))
}

/// `TS`, the reader `cP` gives `managedMcpServers`: each server is read on its own against the entry schema of the
/// field (`pk`), and one whose name the field does not take (`dk`) or that cannot be read is dropped alone, so the map
/// stays even when no server is left. A value that is not an object leaves the key unset.
fn read_managed_mcp_servers(field: &Node, value: &Value) -> Option<Value> {
    let (name_schema, server_schema) = record_of(field)?;
    Some(Value::Object(
        value
            .as_object()?
            .iter()
            .filter_map(|(name, server)| {
                name_schema.validate(Some(&Value::String(name.clone()))).ok()?;
                Some((name.clone(), server_schema.validate(Some(server)).ok()??))
            })
            .collect(),
    ))
}

/// The name and entry schemas of a record schema behind its lazy wrapper.
fn record_of(node: &Node) -> Option<(&Node, &Node)> {
    match node {
        Node::Lazy(inner) => record_of(inner),
        Node::Record { key, value, .. } => Some((key, value)),
        _ => None,
    }
}

/// Lodash `get` (`Et` upstream): the value at `path`.
fn value_at<'a>(settings: &'a Map<String, Value>, path: &[&str]) -> Option<&'a Value> {
    let (first, rest) = path.split_first()?;
    rest.iter().try_fold(settings.get(*first)?, |current, segment| {
        current.as_object()?.get(*segment)
    })
}

/// Lodash `unset`: removes the value at `path`, leaving the objects on the way in place.
fn unset_path(settings: &mut Map<String, Value>, path: &[&str]) -> Option<Value> {
    let (last, parents) = path.split_last()?;
    let mut current = settings;
    for segment in parents {
        current = current.get_mut(*segment)?.as_object_mut()?;
    }
    current.shift_remove(*last)
}

/// `K3`: whether a block read from restrictive values alone stands in for policy content.
fn synthesizes(path: &str) -> bool {
    !POLICY_UNSYNTHESIZED_BLOCKS.contains(&path)
}

/// The schema inside a catch and an optional wrapper.
fn unwrap_catch_optional(node: &Node) -> &Node {
    let node = match node {
        Node::Catch { inner, .. } => &**inner,
        other => other,
    };
    match node {
        Node::Optional(inner) => inner,
        other => other,
    }
}
