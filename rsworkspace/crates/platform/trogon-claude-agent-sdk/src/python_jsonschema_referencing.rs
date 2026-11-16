//! The `referencing` 0.37 registry and resolver `jsonschema` resolves `$ref`, `$dynamicRef` and `$recursiveRef`
//! with: the `jsonschema_specifications` metaschemas, the root schema and everything a crawl of it registers.
//!
//! A URI outside the registry is retrieved as `jsonschema`'s `_REMOTE_WARNING_REGISTRY` retrieves it, through
//! `_warn_for_remote_retrieve`: the [`RemoteSchemaRetriever`] opens it, the `DeprecationWarning` Python emits once the
//! response is open is logged, and the body is parsed like `json.load` into a resource of the specification its
//! `$schema` names, Draft 2020-12 otherwise. Any failure makes the reference `Unresolvable`. Like `referencing`, the
//! retrieved resource joins the registry of the resolver that looked it up and of the resolvers that come from it,
//! and nothing else: another lookup from elsewhere retrieves the URI again.
//!
//! The warning is a `tracing` event at `WARN` with target [`PYTHON_WARNINGS`], carrying the fields of the
//! `warnings.WarningMessage` Python builds: `category`, `message`, `filename` and `lineno` (the frame `stacklevel=9`
//! points at in the Python SDK), with `source` naming the function that warns and `uri` the URI retrieved. Python's
//! default filters hide `DeprecationWarning` outside `__main__`, so a Python process shows nothing; a subscriber
//! that keeps this target sees each warning Python would record with `warnings.catch_warnings(record=True)`. One is
//! emitted for every retrieval, as no `__warningregistry__` dedupes them under `catch_warnings`.

use super::{
    drafts::{self, AnchorsIn, Crazy, Draft, IdOf},
    ops,
};
use crate::constants::{JSON_DEPTH, METASCHEMAS, NO_SUCH_ANCHOR, PYTHON_WARNINGS, REMOTE_RETRIEVE_WARNING};
use crate::{
    RemoteSchemaRetriever,
    python_c_stack::PythonCStack,
    python_hash::HashSecret,
    python_json_load,
    python_json_value::PythonJsonValue as Value,
    python_re::py_repr,
    python_set,
    python_string::PythonString,
    python_urllib::{unquote, urldefrag, urljoin},
    python_value_repr::{repr, type_name},
};
use std::{
    borrow::Cow,
    cell::{OnceCell, RefCell},
    collections::HashMap,
    io::Read,
    rc::Rc,
    sync::LazyLock,
};

/// `jsonschema_specifications.REGISTRY`, already crawled under the secret of this process. No two metaschemas share
/// an id or an anchor, so the order of the crawl does not change what it registers.
pub(super) static SPECIFICATIONS: LazyLock<Result<Crawled<'static>, PythonString>> =
    LazyLock::new(|| specifications(&METASCHEMAS, HashSecret::process()));

/// A schema document and the specification that interprets it (`referencing.Resource`).
#[derive(Clone, Copy, Debug)]
pub(super) struct Resource<'s> {
    pub(super) contents: &'s Value,
    pub(super) draft: Draft,
}

/// An `Anchor` or a `DynamicAnchor` of a crawled registry.
#[derive(Clone, Copy, Debug)]
struct Anchor<'s> {
    resource: Resource<'s>,
    dynamic: bool,
}

/// The resources and anchors a `Registry.crawl()` adds, keyed as `referencing` keys them: by URI, and by URI and
/// anchor name, any of which may hold a lone surrogate.
#[derive(Debug, Default)]
pub(super) struct Crawled<'s> {
    resources: HashMap<PythonString, Resource<'s>>,
    anchors: HashMap<(PythonString, PythonString), Anchor<'s>>,
}

impl<'s> Crawled<'s> {
    /// The document registered under the dialect id of `draft`: the class's `META_SCHEMA`.
    pub(super) fn metaschema(&self, draft: Draft) -> Result<&'s Value, PythonString> {
        let dialect = PythonString::from(draft.dialect());
        match self.resources.get(&dialect) {
            Some(resource) => Ok(resource.contents),
            None => Err(format!("KeyError: {}", py_repr(&dialect)).into()),
        }
    }
}

/// `cls.META_SCHEMA` of the validator class of `draft`, as `jsonschema_specifications` ships it.
pub(super) fn metaschema(draft: Draft) -> Result<&'static Value, PythonString> {
    SPECIFICATIONS.as_ref().map_err(Clone::clone)?.metaschema(draft)
}

#[cfg(test)]
impl Crawled<'_> {
    pub(super) fn resource_count(&self) -> usize {
        self.resources.len()
    }

    pub(super) fn dynamic_anchor_count(&self) -> usize {
        self.anchors.values().filter(|anchor| anchor.dynamic).count()
    }
}

/// An entry of a crawl's work list: a resource, or a string a `subresources_of` generator yielded.
enum Item<'s> {
    Schema(Resource<'s>),
    Text,
}

/// What a lookup raised: a `referencing.exceptions.Unresolvable` (class name and `str()`), or anything else. The
/// text holds the URIs, references and anchor names it names as Python holds them, lone surrogates included.
#[derive(Debug)]
pub(super) enum RefError {
    Unresolvable { class: &'static str, text: PythonString },
    Other(PythonString),
}

impl From<PythonString> for RefError {
    fn from(text: PythonString) -> Self {
        Self::Other(text)
    }
}

impl RefError {
    /// `str()` of the exception as `referencing` raises it.
    pub(super) fn raw(self) -> PythonString {
        match self {
            Self::Unresolvable { text, .. } | Self::Other(text) => text,
        }
    }

    /// `str()` once `_validate_reference` wraps an `Unresolvable` in a `_WrappedReferencingError`.
    pub(super) fn wrapped(self) -> PythonString {
        match self {
            Self::Unresolvable { class, text } => {
                let mut wrapped = PythonString::from(class);
                wrapped.push_str(": ");
                wrapped.extend(text.code_points());
                wrapped
            }
            Self::Other(text) => text,
        }
    }
}

/// `text.rstrip("#")`. A byte below 0x80 is always a whole code point of the encoding, so trimming `#` bytes trims
/// `#` code points.
fn trim_hashes(text: &PythonString) -> PythonString {
    let bytes = text.as_bytes();
    let end = bytes.iter().rposition(|byte| *byte != b'#').map_or(0, |at| at + 1);
    PythonString::from_encoded(bytes[..end].to_vec())
}

/// `text[1:]` when `text.startswith(first)`, for an ASCII `first`.
fn strip_first(text: &PythonString, first: u8) -> Option<PythonString> {
    match text.as_bytes() {
        [lead, rest @ ..] if *lead == first => Some(PythonString::from_encoded(rest.to_vec())),
        _ => None,
    }
}

/// `text.startswith(first)` for an ASCII `first`.
fn starts_with(text: &PythonString, first: u8) -> bool {
    text.as_bytes().first() == Some(&first)
}

/// `text.replace(old, new)` for ASCII `old` and `new`, which never match inside a longer code point.
fn replace(text: &[u8], old: &[u8], new: &[u8]) -> Vec<u8> {
    let mut replaced = Vec::with_capacity(text.len());
    let mut rest = text;
    while let Some((byte, after)) = rest.split_first() {
        rest = match rest.strip_prefix(old) {
            Some(matched) => {
                replaced.extend_from_slice(new);
                matched
            }
            None => {
                replaced.push(*byte);
                after
            }
        };
    }
    replaced
}

fn no_attribute(value: &Value, attribute: &str) -> String {
    format!("'{}' object has no attribute '{attribute}'", type_name(value))
}

/// `AttributeError` of `id_of` on a string the crawl reached through a mapping key or a string's characters.
fn text_has_no_get() -> String {
    "'str' object has no attribute 'get'".to_owned()
}

/// `Resource.id()`: the specification's `id_of`, stripped of trailing `#`.
pub(super) fn id_of(contents: &Value, draft: Draft) -> Result<Option<PythonString>, String> {
    let id = match draft.specification().id_of {
        IdOf::Dollar => {
            if contents.is_boolean() {
                return Ok(None);
            }
            ops::get_some(contents, "$id")?
        }
        IdOf::LegacyDollar => {
            if contents.is_boolean() || ops::has(contents, "$ref")? {
                return Ok(None);
            }
            legacy_id(ops::get_some(contents, "$id")?)?
        }
        IdOf::Legacy => {
            if ops::has(contents, "$ref")? {
                return Ok(None);
            }
            legacy_id(ops::get_some(contents, "id")?)?
        }
    };
    match id {
        None => Ok(None),
        Some(Value::String(id)) => Ok(Some(trim_hashes(id))),
        Some(other) => Err(no_attribute(other, "rstrip")),
    }
}

/// `_legacy_dollar_id` and `_legacy_id` ignore an id that starts with `#`.
fn legacy_id(id: Option<&Value>) -> Result<Option<&Value>, String> {
    match id {
        Some(Value::String(text)) if starts_with(text, b'#') => Ok(None),
        Some(Value::String(_)) | None => Ok(id),
        Some(other) => Err(no_attribute(other, "startswith")),
    }
}

/// `anchors_in`: each anchor name the contents declare and whether it is a `DynamicAnchor`.
fn anchors(contents: &Value, draft: Draft) -> Result<Vec<(Value, bool)>, String> {
    let kind = draft.specification().anchors_in;
    let mut found = Vec::new();
    match kind {
        AnchorsIn::Dollar | AnchorsIn::Dollar2019 => {
            if contents.is_boolean() {
                return Ok(found);
            }
            if let Some(name) = ops::get_some(contents, "$anchor")? {
                found.push((name.clone(), false));
            }
            if kind == AnchorsIn::Dollar
                && let Some(name) = ops::get_some(contents, "$dynamicAnchor")?
            {
                found.push((name.clone(), true));
            }
        }
        AnchorsIn::LegacyDollarId | AnchorsIn::LegacyId => {
            if kind == AnchorsIn::LegacyDollarId && contents.is_boolean() {
                return Ok(found);
            }
            let key = if kind == AnchorsIn::LegacyId { "id" } else { "$id" };
            match ops::get(contents, key)? {
                None => {}
                Some(Value::String(id)) => {
                    if let Some(name) = strip_first(id, b'#') {
                        found.push((Value::String(name), false));
                    }
                }
                Some(other) => return Err(no_attribute(other, "startswith")),
            }
        }
    }
    Ok(found)
}

/// `Resource.subresources()`: pushes, in generator order, each value `subresources_of` yields with the
/// specification `Resource.from_contents` detects for it. The keyword sets iterate in the order `secret` gives them.
fn subresources<'s>(
    uri: &PythonString,
    resource: Resource<'s>,
    stack: &mut Vec<(PythonString, Item<'s>)>,
    secret: HashSecret,
) -> Result<(), String> {
    let Value::Object(map) = resource.contents else {
        return Ok(());
    };
    let spec = resource.draft.specification();
    let mut push = |each: Option<&'s Value>| -> Result<(), String> {
        let item = match each {
            Some(contents) => Item::Schema(Resource {
                contents,
                draft: drafts::detect(contents, resource.draft)?,
            }),
            None => Item::Text,
        };
        stack.push((uri.clone(), item));
        Ok(())
    };
    let in_set = |keys: &'static [&'static str]| {
        python_set::display_order(keys, secret)
            .into_iter()
            .map(move |at| keys[at])
    };
    for key in in_set(spec.in_value) {
        if let Some(value) = map.get(key) {
            push(Some(value))?;
        }
    }
    for key in in_set(spec.in_subarray) {
        if let Some(value) = map.get(key) {
            yield_from(value, &mut push)?;
        }
    }
    for key in in_set(spec.in_subvalues) {
        if let Some(value) = map.get(key) {
            for each in ops::values(value)?.values() {
                push(Some(each))?;
            }
        }
    }
    if spec.crazy == Crazy::Plain {
        return Ok(());
    }
    match map.get("items") {
        None | Some(Value::Null) => {}
        Some(items @ (Value::Array(_) | Value::String(_))) => yield_from(items, &mut push)?,
        Some(items) => push(Some(items))?,
    }
    if spec.crazy == Crazy::Items {
        return Ok(());
    }
    if let Some(dependencies) = ops::get_some(resource.contents, "dependencies")? {
        let mut values = ops::values(dependencies)?.values();
        if let Some(first @ Value::Object(_)) = values.next() {
            push(Some(first))?;
            for each in values {
                push(Some(each))?;
            }
        }
    }
    if spec.crazy == Crazy::AdditionalItemsDependencies {
        for key in ["additionalItems", "additionalProperties"] {
            if let Some(value @ Value::Object(_)) = map.get(key) {
                push(Some(value))?;
            }
        }
    }
    Ok(())
}

/// `yield from value`: list items, or one string per mapping key or character.
fn yield_from<'s>(
    value: &'s Value,
    push: &mut impl FnMut(Option<&'s Value>) -> Result<(), String>,
) -> Result<(), String> {
    match value {
        Value::Array(items) => items.iter().try_for_each(|each| push(Some(each))),
        Value::Object(map) => map.keys().try_for_each(|_| push(None)),
        Value::String(text) => text.code_points().try_for_each(|_| push(None)),
        other => Err(ops::not_iterable(other)),
    }
}

/// `Registry.crawl()` over a work list, last entry first, under the hash secret `secret`.
fn crawl(mut stack: Vec<(PythonString, Item<'_>)>, secret: HashSecret) -> Result<Crawled<'_>, PythonString> {
    let mut crawled = Crawled::default();
    while let Some((uri, item)) = stack.pop() {
        let Item::Schema(resource) = item else {
            return Err(text_has_no_get().into());
        };
        let uri = match id_of(resource.contents, resource.draft)? {
            Some(id) => {
                let uri = urljoin(&uri, &id)?;
                crawled.resources.insert(uri.clone(), resource);
                uri
            }
            None => uri,
        };
        for (name, dynamic) in anchors(resource.contents, resource.draft)? {
            if let Some(error) = ops::unhashable(&name) {
                return Err(error.into());
            }
            if let Some(name) = name.as_python_str() {
                crawled
                    .anchors
                    .insert((uri.clone(), name.clone()), Anchor { resource, dynamic });
            }
        }
        subresources(&uri, resource, &mut stack, secret)?;
    }
    Ok(crawled)
}

/// `jsonschema_specifications.REGISTRY`: each document registered under its id with the specification its
/// `$schema` names, then crawled under `secret`.
pub(super) fn specifications(sources: &[&str], secret: HashSecret) -> Result<Crawled<'static>, PythonString> {
    let mut documents = Vec::with_capacity(sources.len());
    for source in sources {
        documents.push(
            python_json_load::loads_text(source, PythonCStack::MODULE.json_depth())
                .map_err(|error| PythonString::from(error.to_string()))?,
        );
    }
    let documents: &'static [Value] = Box::leak(documents.into_boxed_slice());
    let mut stack = Vec::with_capacity(documents.len());
    for contents in documents {
        let dialect = contents.get("$schema").and_then(Value::as_str).unwrap_or_default();
        let Some(draft) = Draft::with_dialect(dialect.trim_end_matches('#')) else {
            return Err(format!("Could not determine the specification of {}", repr(contents)).into());
        };
        let Some(id) = id_of(contents, draft)? else {
            return Err(format!("{} has no internal id", repr(contents)).into());
        };
        stack.push((id, Item::Schema(Resource { contents, draft })));
    }
    crawl(stack, secret)
}

/// The documents one validation retrieved, kept until it ends. Dropping them does not recurse, so a document nested
/// as deep as `json.load` allows cannot overflow the stack.
#[derive(Default)]
pub(super) struct Documents(typed_arena::Arena<Value>);

impl Documents {
    fn keep(&self, document: Value) -> &Value {
        self.0.alloc(document)
    }
}

impl Drop for Documents {
    fn drop(&mut self) {
        for document in std::mem::take(&mut self.0).into_vec() {
            python_json_load::drop_python_deep(document);
        }
    }
}

/// `registry.crawl().with_resource(uri, resource)` after a retrieval: the crawled registry the retrieval started from
/// (`parent`, or the root registry), plus the retrieved resource, crawled on demand.
struct Retrieved<'s> {
    parent: Option<usize>,
    uri: PythonString,
    resource: Resource<'s>,
    crawled: OnceCell<Result<Crawled<'s>, PythonString>>,
}

impl<'s> Retrieved<'s> {
    /// What crawling this registry under `secret` adds to the crawled registry it came from.
    fn crawl(&self, secret: HashSecret) -> Result<&Crawled<'s>, PythonString> {
        self.crawled
            .get_or_init(|| crawl(vec![(self.uri.clone(), Item::Schema(self.resource))], secret))
            .as_ref()
            .map_err(Clone::clone)
    }
}

/// Which registry a resolver holds: the root registry or one a retrieval made (`node`), crawled or not.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct At {
    node: Option<usize>,
    crawled: bool,
}

impl At {
    /// `registry.crawl()`.
    fn crawled(self) -> Self {
        Self { crawled: true, ..self }
    }
}

/// The registry a validator resolves against: the metaschemas plus the root schema, crawled on demand, and the
/// registries retrievals made from it.
pub(super) struct Registry<'s> {
    specifications: &'static Crawled<'static>,
    root_uri: PythonString,
    root: Resource<'s>,
    crawled: OnceCell<Result<Crawled<'s>, PythonString>>,
    retriever: &'s dyn RemoteSchemaRetriever,
    documents: &'s Documents,
    retrieved: RefCell<Vec<Rc<Retrieved<'s>>>>,
    /// The hash secret of the Python process, which orders the crawls.
    secret: HashSecret,
}

impl<'s> Registry<'s> {
    /// `registry.resolver_with_root(resource)`'s registry over the crawled `specifications`, with the root under
    /// its id or `""`, retrieving through `retriever` into `documents`, in a Python process whose hash secret is
    /// `secret`.
    pub(super) fn new(
        specifications: &'static Crawled<'static>,
        root_uri: PythonString,
        root: Resource<'s>,
        retriever: &'s dyn RemoteSchemaRetriever,
        documents: &'s Documents,
        secret: HashSecret,
    ) -> Self {
        Self {
            specifications,
            root_uri,
            root,
            crawled: OnceCell::new(),
            retriever,
            documents,
            retrieved: RefCell::default(),
            secret,
        }
    }

    fn retrieved(&self, node: usize) -> Option<Rc<Retrieved<'s>>> {
        self.retrieved.borrow().get(node).cloned()
    }

    /// `registry._resources.get(uri)` of the registry at `at`, newest resources first.
    fn resource_at(&self, uri: &PythonString, at: At) -> Result<Option<Resource<'s>>, PythonString> {
        let At { mut node, mut crawled } = at;
        while let Some(retrieved) = node.and_then(|node| self.retrieved(node)) {
            if crawled && let Some(found) = retrieved.crawl(self.secret)?.resources.get(uri) {
                return Ok(Some(*found));
            }
            if retrieved.uri == *uri {
                return Ok(Some(retrieved.resource));
            }
            node = retrieved.parent;
            crawled = true;
        }
        Ok(self.resource(uri, self.overlay(crawled)?))
    }

    /// `registry._anchors.get((uri, name))` of the registry at `at`, newest anchors first.
    fn anchor_at(&self, uri: &PythonString, name: &PythonString, at: At) -> Result<Option<Anchor<'s>>, PythonString> {
        let At { mut node, mut crawled } = at;
        while let Some(retrieved) = node.and_then(|node| self.retrieved(node)) {
            if crawled && let Some(found) = retrieved.crawl(self.secret)?.anchors.get(&(uri.clone(), name.clone())) {
                return Ok(Some(*found));
            }
            node = retrieved.parent;
            crawled = true;
        }
        Ok(self.find_anchor(uri, name, self.overlay(crawled)?))
    }

    /// `_warn_for_remote_retrieve(uri)`: `None` for anything it raises. The URI reaches the retriever as Python
    /// holds it, lone surrogates included, through [`RemoteSchemaRetriever::open_python`].
    fn fetch(&self, uri: &PythonString) -> Option<Resource<'s>> {
        let mut response = self.retriever.open_python(uri).ok()?;
        tracing::warn!(target: PYTHON_WARNINGS, category = "DeprecationWarning", filename = "claude_agent_sdk/__init__.py", lineno = 630, source = "jsonschema.validators._warn_for_remote_retrieve", uri = %uri.to_str_lossy(), message = REMOTE_RETRIEVE_WARNING);
        let mut body = Vec::new();
        response.read_to_end(&mut body).ok()?;
        drop(response);
        let contents = self.documents.keep(python_json_load::loads(&body, JSON_DEPTH)?);
        let draft = drafts::detect(contents, Draft::Draft202012).ok()?;
        Some(Resource { contents, draft })
    }

    /// The rest of `Registry.get_or_retrieve(uri)` once the registry at `at` is crawled and lacks `uri`: the retrieved
    /// resource and the registry it joins, or the `Unresolvable` that `Resolver.lookup` raises for `reference`.
    fn retrieve(&self, uri: &PythonString, at: At, reference: &PythonString) -> Result<(Resource<'s>, At), RefError> {
        let Some(resource) = self.fetch(uri) else {
            return Err(RefError::Unresolvable {
                class: "Unresolvable",
                text: reference.clone(),
            });
        };
        let mut retrieved = self.retrieved.borrow_mut();
        let node = retrieved.len();
        retrieved.push(Rc::new(Retrieved {
            parent: at.node,
            uri: trim_hashes(uri),
            resource,
            crawled: OnceCell::new(),
        }));
        Ok((
            resource,
            At {
                node: Some(node),
                crawled: false,
            },
        ))
    }

    /// `Registry.crawl()`, which only has the root left to crawl and so always builds the same registry.
    fn crawl(&self) -> Result<&Crawled<'s>, PythonString> {
        self.crawled
            .get_or_init(|| crawl(vec![(self.root_uri.clone(), Item::Schema(self.root))], self.secret))
            .as_ref()
            .map_err(Clone::clone)
    }

    /// The crawled overlay when the registry in use is the crawled one.
    fn overlay(&self, crawled: bool) -> Result<Option<&Crawled<'s>>, PythonString> {
        if crawled { self.crawl().map(Some) } else { Ok(None) }
    }

    /// `registry._resources.get(uri)`.
    fn resource(&self, uri: &PythonString, overlay: Option<&Crawled<'s>>) -> Option<Resource<'s>> {
        if let Some(found) = overlay.and_then(|crawled| crawled.resources.get(uri)) {
            return Some(*found);
        }
        if *uri == self.root_uri {
            return Some(self.root);
        }
        self.specifications.resources.get(uri).copied()
    }

    /// `registry._anchors.get((uri, name))`.
    fn find_anchor(
        &self,
        uri: &PythonString,
        name: &PythonString,
        overlay: Option<&Crawled<'s>>,
    ) -> Option<Anchor<'s>> {
        let key = (uri.clone(), name.clone());
        if let Some(found) = overlay.and_then(|crawled| crawled.anchors.get(&key)) {
            return Some(*found);
        }
        self.specifications.anchors.get(&key).copied()
    }

    /// `Registry.anchor(uri, name)` on the registry at `at`, with the registry it answers with.
    fn anchor(&self, uri: &PythonString, name: &PythonString, at: At) -> Result<(Anchor<'s>, At), RefError> {
        if let Some(found) = self.anchor_at(uri, name, at)? {
            return Ok((found, at));
        }
        let all = at.crawled();
        if let Some(found) = self.anchor_at(uri, name, all)? {
            return Ok((found, all));
        }
        let Some(resource) = self.resource_at(&trim_hashes(uri), at)? else {
            return Err(RefError::Other(py_repr(uri).into()));
        };
        // `Registry.crawl` above already ran `resource.id()` on every resource here and raised what it raises.
        if let Some(canonical) = id_of(resource.contents, resource.draft).map_err(PythonString::from)?
            && let Some(found) = self.anchor_at(&canonical, name, all)?
        {
            return Ok((found, all));
        }
        if name.as_bytes().contains(&b'/') {
            let mut text = PythonString::from("'#");
            text.extend(name.code_points());
            text.push_str(
                "' is not a valid anchor, neither as a plain name anchor nor as a JSON Pointer. You may have intended to use '#/",
            );
            text.extend(name.code_points());
            text.push_str("', as the slash is required *before each segment* of a JSON pointer.");
            return Err(RefError::Unresolvable {
                class: "InvalidAnchor",
                text,
            });
        }
        Err(RefError::Unresolvable {
            class: NO_SUCH_ANCHOR,
            text: format!("{} does not exist within {}", py_repr(name), repr(resource.contents)).into(),
        })
    }
}

/// One `uri` of a resolver's dynamic scope.
#[derive(Debug)]
struct Scope {
    uri: PythonString,
    next: Option<Rc<Scope>>,
}

/// A `referencing.Resolver`: its base URI, the URIs it came through and the registry it holds.
#[derive(Clone, Debug)]
pub(super) struct Resolver {
    base: PythonString,
    previous: Option<Rc<Scope>>,
    at: At,
}

/// `referencing.Resolved`.
#[derive(Debug)]
pub(super) struct Resolved<'s> {
    pub(super) contents: Cow<'s, Value>,
    pub(super) resolver: Resolver,
}

impl Resolver {
    pub(super) fn new(base: PythonString) -> Self {
        Self {
            base,
            previous: None,
            at: At::default(),
        }
    }

    /// Whether the registry this resolver holds is crawled, which makes an anchor lookup cheaper.
    pub(super) fn crawled(&self) -> bool {
        self.at.crawled
    }

    /// `Resolver.in_subresource(specification.create_resource(contents))`, `None` when it returns itself.
    fn in_subresource(&self, contents: &Value, draft: Draft) -> Result<Option<Self>, PythonString> {
        let Some(id) = id_of(contents, draft)? else {
            return Ok(None);
        };
        Ok(Some(Self {
            base: urljoin(&self.base, &id)?,
            previous: self.previous.clone(),
            at: self.at,
        }))
    }

    /// `Resolver.in_subresource(specification.create_resource(contents))`.
    pub(super) fn entered(&self, contents: &Value, draft: Draft) -> Result<Self, PythonString> {
        Ok(self.in_subresource(contents, draft)?.unwrap_or_else(|| self.clone()))
    }

    /// `Resolver._evolve(base_uri=base, registry=...)`.
    fn evolve(&self, base: PythonString, at: At) -> Self {
        let previous = if !self.base.is_empty() && (self.previous.is_none() || base != self.base) {
            Some(Rc::new(Scope {
                uri: self.base.clone(),
                next: self.previous.clone(),
            }))
        } else {
            self.previous.clone()
        };
        Self { base, previous, at }
    }

    /// `Resolver.dynamic_scope()`, most recent first.
    fn dynamic_scope(&self) -> impl Iterator<Item = &PythonString> {
        std::iter::successors(self.previous.as_deref(), |scope| scope.next.as_deref()).map(|scope| &scope.uri)
    }

    /// `Resolver.lookup(reference)`.
    pub(super) fn lookup<'s>(&self, registry: &Registry<'s>, reference: &Value) -> Result<Resolved<'s>, RefError> {
        let Value::String(reference) = reference else {
            return Err(RefError::Other(no_attribute(reference, "startswith").into()));
        };
        self.lookup_str(registry, reference)
    }

    fn lookup_str<'s>(&self, registry: &Registry<'s>, reference: &PythonString) -> Result<Resolved<'s>, RefError> {
        let (uri, fragment) = match strip_first(reference, b'#') {
            Some(fragment) => (self.base.clone(), fragment),
            None => urldefrag(&urljoin(&self.base, reference)?)?,
        };
        let (resource, at) = match registry.resource_at(&uri, self.at)? {
            Some(found) => (found, self.at),
            None => {
                let all = self.at.crawled();
                match registry.resource_at(&uri, all)? {
                    Some(found) => (found, all),
                    None => registry.retrieve(&uri, all, reference)?,
                }
            }
        };
        if starts_with(&fragment, b'/') {
            return pointer(resource, &fragment, self.evolve(uri, at));
        }
        if fragment.is_empty() {
            return Ok(Resolved {
                contents: Cow::Borrowed(resource.contents),
                resolver: self.evolve(uri, at),
            });
        }
        let (anchor, at) = registry.anchor(&uri, &fragment, at)?;
        let resolver = self.evolve(uri, at);
        if !anchor.dynamic {
            return Ok(Resolved {
                contents: Cow::Borrowed(anchor.resource.contents),
                resolver,
            });
        }
        resolve_dynamic(registry, anchor.resource, &fragment, resolver)
    }

    /// `referencing.jsonschema.lookup_recursive_ref(resolver)`.
    pub(super) fn lookup_recursive<'s>(&self, registry: &Registry<'s>) -> Result<Resolved<'s>, RefError> {
        let mut resolved = self.lookup_str(registry, &PythonString::from("#"))?;
        if recursive_anchor(&resolved.contents) {
            for uri in self.dynamic_scope() {
                let next = self.lookup_str(registry, uri)?;
                if !recursive_anchor(&next.contents) {
                    break;
                }
                resolved = next;
            }
        }
        Ok(resolved)
    }
}

/// `isinstance(contents, Mapping) and contents.get("$recursiveAnchor")`.
fn recursive_anchor(contents: &Value) -> bool {
    matches!(contents, Value::Object(map) if map.get("$recursiveAnchor").is_some_and(ops::truthy))
}

/// `DynamicAnchor.resolve(resolver)`: the outermost dynamic anchor of the same name in the dynamic scope.
fn resolve_dynamic<'s>(
    registry: &Registry<'s>,
    resource: Resource<'s>,
    name: &PythonString,
    resolver: Resolver,
) -> Result<Resolved<'s>, RefError> {
    let mut last = resource;
    for uri in resolver.dynamic_scope() {
        match registry.anchor(uri, name, resolver.at) {
            Ok((anchor, _)) if anchor.dynamic => last = anchor.resource,
            Ok(_)
            | Err(RefError::Unresolvable {
                class: NO_SUCH_ANCHOR, ..
            }) => {}
            Err(error) => return Err(error),
        }
    }
    let resolver = resolver.entered(last.contents, last.draft)?;
    Ok(Resolved {
        contents: Cow::Borrowed(last.contents),
        resolver,
    })
}

/// `Resource.pointer(pointer, resolver)`.
fn pointer<'s>(
    resource: Resource<'s>,
    pointer: &PythonString,
    mut resolver: Resolver,
) -> Result<Resolved<'s>, RefError> {
    let mut contents = Cow::Borrowed(resource.contents);
    let mut segments: Vec<Option<PythonString>> = Vec::new();
    let unquoted = unquote(&strip_first(pointer, b'/').unwrap_or_default());
    for part in unquoted.as_bytes().split(|byte| *byte == b'/') {
        let (segment, child) = match contents {
            Cow::Borrowed(value) => step(value, part)?,
            Cow::Owned(value) => {
                let (segment, child) = step(&value, part)?;
                (segment, child.map(|child| Cow::Owned(child.into_owned())))
            }
        };
        let Some(child) = child else {
            let hint = if pointer.as_bytes() == b"/" {
                ". The pointer '/' is a valid JSON Pointer but it points to an empty string property ''. If you intended to point to the entire resource, you should use '#'."
            } else {
                ""
            };
            return Err(RefError::Unresolvable {
                class: "PointerToNowhere",
                text: format!(
                    "{} does not exist within {}{hint}",
                    py_repr(pointer),
                    repr(resource.contents)
                )
                .into(),
            });
        };
        segments.push(segment);
        if let Some(entered) = maybe_in_subresource(resource.draft, &segments, &resolver, &child)? {
            resolver = entered;
            segments.clear();
        }
        contents = child;
    }
    Ok(Resolved { contents, resolver })
}

/// One `contents[segment]` of `Resource.pointer`: the segment `maybe_in_subresource` sees (`None` for an
/// integer) and the value, `None` on the `LookupError` that becomes `PointerToNowhere`.
type Step<'a> = (Option<PythonString>, Option<Cow<'a, Value>>);

/// `contents[part]` for `part`, one encoded segment of the unquoted pointer: the pointer is cut at `/` bytes,
/// which always are whole code points.
fn step<'a>(contents: &'a Value, part: &[u8]) -> Result<Step<'a>, PythonString> {
    let part = PythonString::from_encoded(part.to_vec());
    match contents {
        Value::Array(items) => {
            let index = ops::parse_python_int(&part)?;
            let found = ops::position(index, items.len()).and_then(|at| items.get(at));
            Ok((None, found.map(Cow::Borrowed)))
        }
        Value::String(text) => {
            let index = ops::parse_python_int(&part)?;
            let found = ops::position(index, text.len()).and_then(|at| text.code_points().nth(at));
            Ok((None, found.map(|c| Cow::Owned(Value::String(PythonString::from(c))))))
        }
        Value::Object(map) => {
            let key = PythonString::from_encoded(replace(&replace(part.as_bytes(), b"~1", b"/"), b"~0", b"~"));
            let found = map.get(&key).map(Cow::Borrowed);
            Ok((Some(key), found))
        }
        other => Err(format!("'{}' object is not subscriptable", type_name(other)).into()),
    }
}

/// `specification.maybe_in_subresource(segments, resolver, subresource)`, `None` when it returns `resolver`.
fn maybe_in_subresource(
    draft: Draft,
    segments: &[Option<PythonString>],
    resolver: &Resolver,
    contents: &Value,
) -> Result<Option<Resolver>, PythonString> {
    let spec = draft.specification();
    let crazy: &[&str] = match spec.crazy {
        Crazy::Plain => &[],
        Crazy::Items => &["items"],
        Crazy::ItemsDependencies | Crazy::AdditionalItemsDependencies => &["items", "dependencies"],
    };
    // A segment holding a lone surrogate names no keyword, like an integer one.
    let mut rest = segments
        .iter()
        .map(|segment| segment.as_ref().and_then(PythonString::as_str));
    while let Some(segment) = rest.next() {
        let named = |set: &[&str]| segment.is_some_and(|name| set.contains(&name));
        if named(crazy) && contents.is_object() {
            return resolver.in_subresource(contents, draft);
        }
        if !named(spec.maybe_in_value) && (!named(spec.maybe_in_child) || rest.next().is_none()) {
            return Ok(None);
        }
    }
    resolver.in_subresource(contents, draft)
}
