//! The reader `cP` gives `sandbox.credentials` in a managed document (`R` upstream).
//!
//! A credential entry that cannot be read is never allowed to unprotect what it names: a file or variable entry
//! keeps blocking it, an AWS pair keeps suppressing the implicit pairing under names that re-sign nothing, and a
//! SigV4 shape stays denied. A block that is not an object at all reads as the fail-closed skeleton.

use std::collections::HashSet;

use serde_json::{Map, Value};

use super::{Node, object_of, prepare};
use crate::constants::{
    AWS_CREDENTIAL_VARIABLES, POLICY_FILE_HASH_OFFSET, POLICY_FILE_HASH_PRIME, POLICY_INVALID_PAIR_PREFIX,
    SETTINGS_AWS_PAIR_SLOTS,
};

/// The schemas the reader takes its entries from, and the state one document shares across them (`E`, `b` and
/// `C` upstream).
struct Reader<'a> {
    files: &'a Node,
    env_vars: &'a Node,
    plaintext: &'a Node,
    pair: &'a Node,
    name: &'a Node,
    sigv4: &'a Node,
    prefix: String,
    count: usize,
    pending: Vec<Value>,
    stood_in: HashSet<&'static str>,
}

/// The block read from a managed document, with whether it is made up only of values put in place of what was
/// written. `None` when the schema has no credentials block to read.
pub(super) fn read(credentials: &Node, file: Option<&str>, input: &Value) -> Option<(Value, bool)> {
    let members = object_of(credentials)?.members;
    let member = |name: &str| members.iter().find(|(key, _)| key == name).map(|(_, node)| &**node);
    let pair = element_of(member("awsPairs")?)?;
    let mut reader = Reader {
        files: element_of(member("files")?)?,
        env_vars: element_of(member("envVars")?)?,
        plaintext: member("allowPlaintextInject")?,
        pair,
        name: object_of(pair)?
            .members
            .iter()
            .find(|(key, _)| key == "accessKeyIdVar")
            .map(|(_, node)| &**node)?,
        sigv4: member("sigv4")?,
        prefix: file_prefix(file),
        count: 0,
        pending: Vec::new(),
        stood_in: HashSet::new(),
    };
    let Value::Object(fields) = input else {
        return Some((Value::Object(reader.skeleton()), true));
    };
    let output = reader.fields(fields);
    let synthesized = !output.is_empty() && output.keys().all(|key| reader.stood_in.contains(key.as_str()));
    Some((Value::Object(output), synthesized))
}

/// The element schema of an optional array.
fn element_of(node: &Node) -> Option<&Node> {
    match node {
        Node::Optional(inner) => element_of(inner),
        Node::Array { element, .. } => Some(&**element),
        _ => None,
    }
}

/// `w`: the FNV-1a hash of the file the document came from, which keeps the names of stand-in pairs from
/// different files apart. Each code point contributes its first UTF-16 unit.
fn file_prefix(file: Option<&str>) -> String {
    file.map_or_else(String::new, |file| {
        let hash = file.chars().fold(POLICY_FILE_HASH_OFFSET, |hash, character| {
            let code = u32::from(character);
            let unit = if code < 0x1_0000 {
                code
            } else {
                0xD800 + ((code - 0x1_0000) >> 10)
            };
            (hash ^ unit).wrapping_mul(POLICY_FILE_HASH_PRIME)
        });
        format!("{hash:08X}_")
    })
}

/// The SigV4 block with every shape denied.
fn all_denied(sigv4: &Node) -> Value {
    let shapes = object_of(sigv4).map_or(&[][..], |shape| shape.members);
    Value::Object(
        shapes
            .iter()
            .map(|(name, _)| (name.clone(), Value::String("deny".to_owned())))
            .collect(),
    )
}

impl Reader<'_> {
    /// The fields of an object block, in the order of the schema. Unknown keys are dropped.
    fn fields(&mut self, fields: &Map<String, Value>) -> Map<String, Value> {
        let mut output = Map::new();
        if let Some(written) = fields.get("files") {
            let files = self.files;
            output.insert("files".to_owned(), self.entries("files", files, "path", written));
        }
        if let Some(written) = fields.get("envVars") {
            let env_vars = self.env_vars;
            output.insert("envVars".to_owned(), self.entries("envVars", env_vars, "name", written));
        }
        if let Some(written) = fields.get("allowPlaintextInject") {
            let read = self
                .plaintext
                .validate(prepare(Some(&Value::Bool(false)), Some(written)).as_ref());
            let value = read.ok().flatten().unwrap_or_else(|| {
                self.stood_in.insert("allowPlaintextInject");
                Value::Bool(false)
            });
            output.insert("allowPlaintextInject".to_owned(), value);
        }
        if let Some(written) = fields.get("awsPairs") {
            output.insert("awsPairs".to_owned(), self.pairs(written));
        }
        if let Some(written) = fields.get("sigv4") {
            output.insert("sigv4".to_owned(), self.signing(written));
        }
        output
    }

    /// The fail-closed skeleton of a block that is not an object: no plaintext injection, the implicit AWS
    /// pairing suppressed, and every SigV4 shape denied.
    fn skeleton(&mut self) -> Map<String, Value> {
        let suppressors = self.conventional_suppressors();
        let mut output = Map::new();
        output.insert("allowPlaintextInject".to_owned(), Value::Bool(false));
        output.insert("awsPairs".to_owned(), Value::Array(suppressors));
        output.insert("sigv4".to_owned(), all_denied(self.sigv4));
        output
    }

    /// `M`: `files` and `envVars`. An entry that cannot be read but names what it protects is kept as a deny;
    /// any other unreadable entry is ignored. A lone entry object reads as a one-element list.
    fn entries(&mut self, key: &'static str, element: &Node, target: &str, written: &Value) -> Value {
        let read = |entry: &Value| element.validate(Some(entry)).ok().flatten();
        let kept = match written {
            Value::Array(items) => {
                let kept: Vec<Value> = items
                    .iter()
                    .filter_map(|entry| read(entry).or_else(|| degrade(element, target, entry)))
                    .collect();
                if !items.is_empty() && kept.is_empty() {
                    self.stood_in.insert(key);
                }
                kept
            }
            Value::Object(_) => match read(written).or_else(|| degrade(element, target, written)) {
                Some(entry) => vec![entry],
                None => self.ignored(key),
            },
            _ => self.ignored(key),
        };
        Value::Array(kept)
    }

    /// A field read as nothing, which counts as a stand-in.
    fn ignored(&mut self, key: &'static str) -> Vec<Value> {
        self.stood_in.insert(key);
        Vec::new()
    }

    /// `awsPairs`. A pair that cannot be read but claims a conventional AWS name becomes a suppressor; a value
    /// that is not a list becomes the pair it holds, or suppressors for every conventional name.
    fn pairs(&mut self, written: &Value) -> Value {
        if let Value::Array(items) = written {
            let mut kept = Vec::new();
            for entry in items {
                let read = match self.pair.validate(Some(entry)) {
                    Ok(Some(pair)) => Some(pair),
                    _ => self.suppressor(entry, true),
                };
                kept.extend(read);
            }
            kept.append(&mut self.pending);
            if !items.is_empty() && kept.is_empty() {
                self.stood_in.insert("awsPairs");
            }
            return Value::Array(kept);
        }
        let lone = self.suppressor(written, false);
        let claims_pair = written
            .as_object()
            .is_some_and(|fields| SETTINGS_AWS_PAIR_SLOTS.iter().any(|slot| fields.contains_key(*slot)));
        if lone.is_none() && claims_pair {
            self.pending.clear();
            return Value::Array(self.ignored("awsPairs"));
        }
        let mut kept = match lone {
            Some(pair) => vec![pair],
            None => {
                self.stood_in.insert("awsPairs");
                self.conventional_suppressors()
            }
        };
        kept.append(&mut self.pending);
        Value::Array(kept)
    }

    /// One suppressor for each conventional AWS name.
    fn conventional_suppressors(&mut self) -> Vec<Value> {
        AWS_CREDENTIAL_VARIABLES
            .iter()
            .filter_map(|name| self.suppressor(&single_slot(name), false))
            .collect()
    }

    /// `x`: the pair kept from an entry that cannot be read, with every name it cannot keep replaced by one that
    /// re-signs nothing. An entry claiming no conventional AWS name is ignored. A pair whose names could all be
    /// kept loses its secret name, and a conventional secret name gets a suppressor of its own. `failed` says
    /// whether the entry failed the pair schema, which makes a slot holding something other than a string read
    /// as blank.
    fn suppressor(&mut self, entry: &Value, failed: bool) -> Option<Value> {
        let fields = entry.as_object()?;
        let slot = |key: &str, required: bool| match fields.get(key) {
            Some(Value::String(name)) => Some(name.clone()),
            Some(_) if failed => Some(String::new()),
            None if failed && required => Some(String::new()),
            _ => None,
        };
        let access = slot("accessKeyIdVar", true);
        let secret = slot("secretAccessKeyVar", true);
        let session = slot("sessionTokenVar", false);
        let conventional = |name: &Option<String>| {
            name.as_deref()
                .is_some_and(|name| AWS_CREDENTIAL_VARIABLES.contains(&name))
        };
        if ![&access, &secret, &session].into_iter().any(conventional) {
            return None;
        }
        let valid = |name: &Option<String>| {
            name.as_ref()
                .is_some_and(|name| self.name.validate(Some(&Value::String(name.clone()))).is_ok())
        };
        let (access_ok, secret_ok, session_ok) = (valid(&access), valid(&secret), valid(&session));
        self.count += 1;
        let stand_in = |kind: &str| format!("{POLICY_INVALID_PAIR_PREFIX}{kind}_{}{}_", self.prefix, self.count);
        let access = access
            .filter(|_| access_ok)
            .unwrap_or_else(|| stand_in("ACCESS_KEY_ID"));
        let secret = secret
            .filter(|name| secret_ok && *name != access)
            .unwrap_or_else(|| stand_in("SECRET_ACCESS_KEY"));
        let session = session.map(|name| {
            if session_ok && name != access && name != secret {
                name
            } else {
                stand_in("SESSION_TOKEN")
            }
        });
        let mut secret = secret;
        if !access.starts_with(POLICY_INVALID_PAIR_PREFIX) && !secret.starts_with(POLICY_INVALID_PAIR_PREFIX) {
            let claimed = AWS_CREDENTIAL_VARIABLES
                .contains(&secret.as_str())
                .then(|| secret.clone());
            secret = stand_in("SECRET_ACCESS_KEY");
            if let Some(pair) = claimed.and_then(|name| self.suppressor(&single_slot(&name), false)) {
                self.pending.push(pair);
            }
        }
        let mut pair = Map::new();
        pair.insert("accessKeyIdVar".to_owned(), Value::String(access));
        pair.insert("secretAccessKeyVar".to_owned(), Value::String(secret));
        if let Some(session) = session {
            pair.insert("sessionTokenVar".to_owned(), Value::String(session));
        }
        self.pair.validate(Some(&Value::Object(pair))).ok().flatten()
    }

    /// `sigv4`. A shape that cannot be read is denied; a value that is not an object denies every shape.
    fn signing(&mut self, written: &Value) -> Value {
        let (Value::Object(fields), Some(shape)) = (written, object_of(self.sigv4)) else {
            self.stood_in.insert("sigv4");
            return all_denied(self.sigv4);
        };
        let mut output = Map::new();
        let mut denied = HashSet::new();
        for (name, node) in shape.members {
            match node.validate(fields.get(name)) {
                Ok(Some(value)) => {
                    output.insert(name.clone(), value);
                }
                Ok(None) => {}
                Err(()) => {
                    denied.insert(name.as_str());
                    output.insert(name.clone(), Value::String("deny".to_owned()));
                }
            }
        }
        if !output.is_empty() && output.keys().all(|name| denied.contains(name.as_str())) {
            self.stood_in.insert("sigv4");
        }
        Value::Object(output)
    }
}

/// `{accessKeyIdVar: name}`: the entry a suppressor for one conventional name is read from.
fn single_slot(name: &str) -> Value {
    let mut fields = Map::new();
    fields.insert("accessKeyIdVar".to_owned(), Value::String(name.to_owned()));
    Value::Object(fields)
}

/// The deny a file or variable entry that cannot be read is kept as, when it names what it protects with a mode
/// that protects it.
fn degrade(element: &Node, target: &str, entry: &Value) -> Option<Value> {
    let fields = entry.as_object()?;
    let mode = fields.get("mode").and_then(Value::as_str)?;
    let named = fields.get(target).filter(|value| value.is_string())?;
    if mode != "mask" && mode != "deny" {
        return None;
    }
    let mut deny = Map::new();
    deny.insert(target.to_owned(), named.clone());
    deny.insert("mode".to_owned(), Value::String("deny".to_owned()));
    element.validate(Some(&Value::Object(deny))).ok().flatten()
}

#[cfg(test)]
#[path = "settings_schema_policy_credentials_tests.rs"]
mod tests;
