//! The `jsonschema` 4.26 validators the Python SDK bridge validates tool arguments with
//! (`jsonschema.validate(instance=arguments, schema=schema)`), for drafts 3, 4, 6, 7, 2019-09 and 2020-12, with
//! their `referencing` 0.37 reference resolution and the `best_match` error selection that `validate` raises.
//! `validate` first runs `check_schema`: the draft's metaschema validates the schema with the class
//! `FORMAT_CHECKER`, and its first error is raised as a `SchemaError`. The instance itself is validated without a
//! `FormatChecker`, so `format` is an annotation there.
//!
//! The engine follows `Validator.iter_errors` and `Validator.descend` step by step, so the errors come out in
//! Python's order and an exception surfaces at the point Python raises it. Python's recursion limit is a frame
//! counter: each step below charges the frames its Python counterpart pushes on CPython 3.12, measured against
//! CPython, and a frame past the limit of the call's [`RecursionModel`] raises the `RecursionError` Python raises.
//! `check_schema` and the validation share the caches that change those frames, as one Python process does: the
//! `isinstance` caches of the ABCs `jsonschema` checks against and the compiled patterns of the `re` module.
//! `str()` of the error the bridge reports pretty-prints the schema and the instance in the bridge's handler; when
//! that passes the limit the `RecursionError` escapes the handler, which [`Outcome::Escaped`] reports.
//!
//! `validate` resolves with `jsonschema`'s default registry, `_REMOTE_WARNING_REGISTRY`, which retrieves a `$ref`
//! outside the schema and the metaschemas over `urlopen` and warns with a `DeprecationWarning`; the bridge does the
//! same through its [`RemoteSchemaRetriever`]. The validation runs on its own thread, which retrieves too: the
//! retriever is `Sync`, the registry holding what it retrieved never leaves the thread, and the thread runs under
//! the caller's `tracing` dispatcher so the warnings reach the caller's subscriber.

#[path = "python_jsonschema_drafts.rs"]
mod drafts;
#[path = "python_jsonschema_keywords.rs"]
mod keywords;
#[path = "python_jsonschema_ops.rs"]
mod ops;
#[path = "python_jsonschema_referencing.rs"]
mod referencing;

pub(crate) use drafts::Draft;

use crate::constants::{
    BUILTIN_CALL, CHECK_SCHEMA_HELD, COLD_ABC_FRAMES, RECURSION_ERROR, RESUMED, STACK_SIZES, VALIDATE_HELD, WORDS,
};
use crate::{
    RemoteSchemaError, RemoteSchemaRetriever,
    python_hash::HashSecret,
    python_pprint::{Dedented, PathSegment, pretty_frames, schema_error_text, unknown_type_text},
    python_re_cache::ReCache,
    python_recursion_model::{CacheState, RecursionModel},
    python_value_repr::{repr, type_name},
};
use crate::{python_json_value::PythonJsonValue as Value, python_string::PythonString};
use drafts::Keyword;
use ops::Op;
use referencing::{Registry, Resolver, Resource};
use std::{
    cell::{Cell, RefCell},
    collections::{HashSet, VecDeque},
    io::Read,
    rc::Rc,
    sync::{Mutex, PoisonError},
};

/// What `jsonschema.validate` does with an instance.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Outcome {
    Valid,
    /// A `ValidationError`, whose `message` the bridge reports.
    Invalid(String),
    /// Any other exception, which the bridge reports as `str(error)`; a `SchemaError` is one. An `re.error` quotes
    /// the pattern as it is, so the text may hold a lone surrogate.
    Raised(PythonString),
    /// A `RecursionError` that `str(error)` raised in the bridge's handler, which escapes the handler: the MCP
    /// server answers the request with a JSON-RPC error carrying this message.
    Escaped(String),
}

/// A `jsonschema` instruction that pays C recursion checks only the first time the process executes it: CPython
/// 3.12 runs it generic once and specialized after, and an execution nested in the first one already finds it
/// specialized. A generic `FOR_ITER` resumes the generator through C ([`RESUMED`]) until it yields or returns,
/// and a generic `CALL` of a fast builtin checks once ([`BUILTIN_CALL`]).
#[derive(Clone, Copy, Debug)]
enum Site {
    /// `for error in validator.iter_errors(schema)` of `check_schema`.
    CheckSchema,
    /// `for error in errors` of `iter_errors`.
    IterErrors,
    /// `for error in errors` of `descend`.
    Descend,
    /// `sorted(...)` of `_utils.uniq`.
    Uniq,
    /// `next(...)` of `Validator.is_valid`.
    IsValid,
    /// `next(...)` of `_utils.is_valid`, which `find_evaluated_property_keys_by_schema` calls.
    UtilsIsValid,
    /// `next(validator.descend(...), None)` of `_utils.find_evaluated_item_indexes_by_schema`.
    EvaluatedItems,
    /// `next(validator.descend(...), None)` of `_legacy_keywords.find_evaluated_item_indexes_by_schema`.
    EvaluatedItemsDraft2019,
    /// `next(validator.descend(...), None)` of `_legacy_keywords.find_evaluated_property_keys_by_schema`.
    EvaluatedKeysDraft2019,
    /// `for _ in validator.descend(...)` of `_keywords.unevaluatedProperties`.
    UnevaluatedProperties,
    /// `for _ in validator.descend(...)` of `_legacy_keywords.unevaluatedProperties_draft2019`.
    UnevaluatedPropertiesDraft2019,
}

impl Site {
    /// The bit of [`ReCache::executed`] that says the process executed the instruction.
    fn bit(self) -> u16 {
        1 << self as u16
    }
}

/// A retriever that opens nothing, as a `referencing.Registry` without `retrieve` raises `NoSuchResource`: every
/// reference outside the registry is `Unresolvable`.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct NoRetrieval;

impl RemoteSchemaRetriever for NoRetrieval {
    fn open(&self, uri: &str) -> Result<Box<dyn Read + '_>, RemoteSchemaError> {
        Err(RemoteSchemaError::new("NoSuchResource", uri))
    }
}

/// [`validate_retrieving`] with nothing retrieved, in a process whose `re` caches hold nothing and whose hash secret
/// is the one `PYTHONHASHSEED=0` gives.
#[cfg_attr(
    not(test),
    allow(dead_code, reason = "the bridge validates through validate_retrieving")
)]
pub(crate) fn validate(schema: &Value, instance: &Value) -> Outcome {
    validate_retrieving(schema, instance, &NoRetrieval, &Mutex::default(), HashSecret::default())
}

/// `jsonschema.validate`: `validator_for(schema)` picks the class, `cls.check_schema(schema)` raises the
/// `SchemaError` of an invalid schema, and the `best_match` of every error of the instance is raised, with every
/// remote `$ref` retrieved through `retriever`, on the default [`RecursionModel`]. `patterns` is the process's `re`
/// caches, which the call reads and leaves as Python leaves them; it stays locked for the call, as one Python
/// thread validates at a time. `secret` is the process's hash secret, which orders the sets `jsonschema` and
/// `referencing` iterate.
pub(crate) fn validate_retrieving(
    schema: &Value,
    instance: &Value,
    retriever: &dyn RemoteSchemaRetriever,
    patterns: &Mutex<ReCache>,
    secret: HashSecret,
) -> Outcome {
    validate_with(
        schema,
        instance,
        &RecursionModel::default(),
        retriever,
        patterns,
        secret,
    )
}

/// [`validate_retrieving`] where the recursion limit sits as `model` says.
pub(crate) fn validate_with(
    schema: &Value,
    instance: &Value,
    model: &RecursionModel,
    retriever: &dyn RemoteSchemaRetriever,
    patterns: &Mutex<ReCache>,
    secret: HashSecret,
) -> Outcome {
    validate_on(&STACK_SIZES, schema, instance, model, retriever, patterns, secret)
}

/// [`validate_with`] on a thread whose stack is the first of `stack_sizes` the system grants, under the caller's
/// `tracing` dispatcher, or on the calling thread where it grants none.
fn validate_on(
    stack_sizes: &[usize],
    schema: &Value,
    instance: &Value,
    model: &RecursionModel,
    retriever: &dyn RemoteSchemaRetriever,
    patterns: &Mutex<ReCache>,
    secret: HashSecret,
) -> Outcome {
    let dispatch = tracing::dispatcher::get_default(Clone::clone);
    let work = || {
        tracing::dispatcher::with_default(&dispatch, || {
            validate_locked(schema, instance, model, retriever, patterns, secret)
        })
    };
    let inline = || validate_locked(schema, instance, model, retriever, patterns, secret);
    let panicked = Outcome::Raised("internal error: the validation thread panicked".into());
    crate::large_stack::on_large_stack(stack_sizes, work, inline, panicked)
}

/// [`validate_inline`] with the `re` caches of `patterns`, which hold what the call compiled when it returns, and the
/// hash secret `secret`.
fn validate_locked(
    schema: &Value,
    instance: &Value,
    model: &RecursionModel,
    retriever: &dyn RemoteSchemaRetriever,
    patterns: &Mutex<ReCache>,
    secret: HashSecret,
) -> Outcome {
    let mut caches = patterns.lock().unwrap_or_else(PoisonError::into_inner);
    let mut budget = Budget::new(*model, std::mem::take(&mut *caches), secret);
    let outcome = validate_inline(schema, instance, retriever, &mut budget);
    *caches = budget.patterns.into_inner();
    outcome
}

fn validate_inline(
    schema: &Value,
    instance: &Value,
    retriever: &dyn RemoteSchemaRetriever,
    budget: &mut Budget,
) -> Outcome {
    let draft = match drafts::validator_for(schema, Draft::Draft202012) {
        Ok(draft) => draft,
        Err(text) => return Outcome::Raised(text),
    };
    if let Err(outcome) = check_schema_spending(draft, schema, budget) {
        return outcome;
    }
    let (errors, raised) = iter_errors_spending(draft, schema, instance, budget, retriever);
    select(&errors, raised.map(|text| budget.raised(text)))
}

/// [`best_match_retrieving`] with nothing retrieved.
#[cfg_attr(
    not(test),
    allow(dead_code, reason = "the bridge validates through validate_retrieving")
)]
pub(crate) fn best_match(draft: Draft, schema: &Value, instance: &Value) -> Outcome {
    best_match_retrieving(draft, schema, instance, &NoRetrieval)
}

/// `best_match(cls(schema).iter_errors(instance))` raised when there is one, the last step of `validate`.
fn best_match_retrieving(
    draft: Draft,
    schema: &Value,
    instance: &Value,
    retriever: &dyn RemoteSchemaRetriever,
) -> Outcome {
    let mut budget = Budget::default();
    let (errors, raised) = iter_errors_spending(draft, schema, instance, &mut budget, retriever);
    select(&errors, raised.map(|text| budget.raised(text)))
}

/// `cls.check_schema(schema)` for the validator class of `draft`: `validator_for(cls.META_SCHEMA, default=cls)`
/// validates the schema against `cls.META_SCHEMA` with its `FORMAT_CHECKER`, and the first error `iter_errors`
/// yields (not the `best_match`) is raised as `SchemaError.create_from(error)`. Every `$ref` of a metaschema resolves
/// within the metaschemas, so nothing is retrieved.
#[cfg(test)]
pub(crate) fn check_schema(draft: Draft, schema: &Value) -> Result<(), Outcome> {
    check_schema_spending(draft, schema, &mut Budget::default())
}

/// `check_schema` spending `budget`, whose caches the validation that follows goes on using.
fn check_schema_spending(draft: Draft, schema: &Value, budget: &mut Budget) -> Result<(), Outcome> {
    let metaschema = referencing::metaschema(draft).map_err(Outcome::Raised)?;
    let class = drafts::validator_for(metaschema, draft).map_err(Outcome::Raised)?;
    let mut sink = Sink::new(true);
    if let Err(Stop::Raise(text)) = run(class, metaschema, schema, true, &mut sink, &NoRetrieval, budget) {
        return Err(budget.raised(text));
    }
    match sink.errors.first() {
        None => Ok(()),
        Some(error) => Err(schema_error(error, budget)),
    }
}

/// `str(SchemaError.create_from(error))`: the message alone while a field `__str__` needs is still `_unset`, and
/// [`Outcome::Escaped`] when printing it passes the limit of `budget`'s model, which leaves in the `re` caches what
/// the printing compiled.
fn schema_error(error: &PyError, budget: &Budget) -> Outcome {
    let (Some(validator), Some(_), Some(instance), Some(schema)) =
        (&error.validator, &error.validator_value, &error.instance, &error.schema)
    else {
        return Outcome::Raised(error.message.as_str().into());
    };
    let text = || {
        let schema_path = Vec::from(error.schema_path.clone());
        let path = Vec::from(error.path.clone());
        schema_error_text(
            &error.message,
            validator.as_deref(),
            &schema_path,
            "metaschema",
            schema,
            &path,
            "schema",
            instance,
        )
    };
    match printed(&budget.model, &budget.patterns, schema, instance, text) {
        Some(text) => Outcome::Raised(text.into()),
        None => Outcome::Escaped(RECURSION_ERROR.to_owned()),
    }
}

/// `list(cls(schema).iter_errors(instance))` for the validator class of `draft`: every error in Python's order,
/// and the `str()` of the exception that stopped the iteration, if one did.
#[cfg_attr(
    not(test),
    allow(dead_code, reason = "the bridge validates through validate_retrieving")
)]
pub(crate) fn iter_errors(draft: Draft, schema: &Value, instance: &Value) -> (Vec<PyError>, Option<PythonString>) {
    iter_errors_retrieving(draft, schema, instance, &NoRetrieval)
}

/// [`iter_errors`] with every remote `$ref` retrieved through `retriever`.
fn iter_errors_retrieving(
    draft: Draft,
    schema: &Value,
    instance: &Value,
    retriever: &dyn RemoteSchemaRetriever,
) -> (Vec<PyError>, Option<PythonString>) {
    iter_errors_spending(draft, schema, instance, &mut Budget::default(), retriever)
}

/// [`iter_errors_retrieving`] spending `budget`.
fn iter_errors_spending(
    draft: Draft,
    schema: &Value,
    instance: &Value,
    budget: &mut Budget,
    retriever: &dyn RemoteSchemaRetriever,
) -> (Vec<PyError>, Option<PythonString>) {
    let mut sink = Sink::new(false);
    let raised = match run(draft, schema, instance, false, &mut sink, retriever, budget) {
        Ok(()) | Err(Stop::Done) => None,
        Err(Stop::Raise(text)) => Some(text),
    };
    (sink.errors, raised)
}

/// `cls(schema, format_checker=...)` with no registry, then its `iter_errors(instance)`; `format_checker` says
/// whether the class `FORMAT_CHECKER` is passed, as `check_schema` does, or none is. A `$ref` outside the registry
/// is retrieved through `retriever`. The iteration starts at the first frame of `budget`'s model and spends its
/// caches.
fn run(
    draft: Draft,
    schema: &Value,
    instance: &Value,
    format_checker: bool,
    sink: &mut Sink,
    retriever: &dyn RemoteSchemaRetriever,
    budget: &mut Budget,
) -> Flow {
    let uri = referencing::id_of(schema, draft)?.unwrap_or_default();
    let documents = referencing::Documents::default();
    let specifications = referencing::SPECIFICATIONS.as_ref().map_err(Clone::clone)?;
    let registry = Registry::new(
        specifications,
        uri.clone(),
        Resource {
            contents: schema,
            draft,
        },
        retriever,
        &documents,
        budget.secret,
    );
    let engine = Engine {
        registry,
        depth: Cell::new(budget.model.first_frame()),
        budget: std::mem::take(budget),
        format_checker,
        held: Cell::new(VALIDATE_HELD),
        steps: RefCell::new(Vec::new()),
    };
    if format_checker {
        // The first execution of the loop holds until the first error, which ends `check_schema`.
        let first = if engine.first(Site::CheckSchema) { RESUMED } else { 0 };
        engine.held.set(CHECK_SCHEMA_HELD + first);
    }
    let flow = Validator::new(draft, schema, Resolver::new(uri))
        .map_err(Stop::from)
        .and_then(|validator| engine.iter_errors(&validator, instance, sink));
    *budget = engine.budget;
    flow
}

/// A `jsonschema.exceptions.ValidationError`. A field is `None` while it is still `_unset`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PyError {
    pub(crate) message: String,
    /// `error.path`, outermost step first.
    pub(crate) path: VecDeque<PathSegment>,
    /// `error.schema_path`, outermost step first.
    pub(crate) schema_path: VecDeque<PathSegment>,
    pub(crate) context: Vec<PyError>,
    /// `error.validator`; `Some(None)` is the `None` of a false schema.
    pub(crate) validator: Option<Option<String>>,
    /// `error.validator_value`; JSON `null` stands for Python's `None`.
    pub(crate) validator_value: Option<Value>,
    pub(crate) instance: Option<Value>,
    pub(crate) schema: Option<Value>,
    /// The draft whose `TYPE_CHECKER` `error._type_checker` is.
    pub(crate) type_checker: Option<Draft>,
}

impl PyError {
    fn new(message: String) -> Self {
        Self {
            message,
            path: VecDeque::new(),
            schema_path: VecDeque::new(),
            context: Vec::new(),
            validator: None,
            validator_value: None,
            instance: None,
            schema: None,
            type_checker: None,
        }
    }

    /// `ValidationError(message, validator=keyword, validator_value=value)`.
    fn keyword(message: String, keyword: &str, value: &Value) -> Self {
        Self {
            validator: Some(Some(keyword.to_owned())),
            validator_value: Some(value.clone()),
            ..Self::new(message)
        }
    }

    /// `ValidationError(message, context=context)`.
    fn with_context(message: String, context: Vec<PyError>) -> Self {
        Self {
            context,
            ..Self::new(message)
        }
    }

    /// `ValidationError(f"False schema does not allow {instance!r}", validator=None, validator_value=None, ...)`,
    /// made with `held` C recursion checks under way.
    fn false_schema(instance: &Value, schema: &Value, held: usize) -> Result<Self, String> {
        Ok(Self {
            validator: Some(None),
            validator_value: Some(Value::Null),
            instance: Some(instance.clone()),
            schema: Some(schema.clone()),
            ..Self::new(format!("False schema does not allow {}", ops::shown(instance, held)?))
        })
    }

    /// `error._set(validator=keyword, validator_value=value, instance=instance, schema=schema,
    /// type_checker=checker)`: only the fields still unset change.
    fn set(&mut self, keyword: &str, value: &Value, instance: &Value, schema: &Value, checker: Draft) {
        self.type_checker.get_or_insert(checker);
        self.validator.get_or_insert_with(|| Some(keyword.to_owned()));
        self.validator_value.get_or_insert_with(|| value.clone());
        self.instance.get_or_insert_with(|| instance.clone());
        self.schema.get_or_insert_with(|| schema.clone());
    }
}

/// Why an error iteration ended early.
#[derive(Debug)]
enum Stop {
    /// The consumer only wanted the first error and has it.
    Done,
    /// An exception, as its `str()`.
    Raise(PythonString),
}

impl From<PythonString> for Stop {
    fn from(text: PythonString) -> Self {
        Self::Raise(text)
    }
}

/// An exception whose `str()` is built from reprs, so it holds no lone surrogate.
impl From<String> for Stop {
    fn from(text: String) -> Self {
        Self::Raise(text.into())
    }
}

/// The result of producing errors into a [`Sink`].
type Flow = Result<(), Stop>;

/// The consumer of an error generator: `list(...)`, or `next(...)` when `first` is set.
struct Sink {
    errors: Vec<PyError>,
    first: bool,
    /// The first executions of a `FOR_ITER` that an error yielded here passes through, each set once one did.
    steps: Vec<Rc<Cell<bool>>>,
}

impl Sink {
    fn new(first: bool) -> Self {
        Self {
            errors: Vec::new(),
            first,
            steps: Vec::new(),
        }
    }

    /// A sink whose errors `self` yields in turn, as `yield from` and `for error in errors: yield error` do, also
    /// passing through `step` when there is one.
    fn forwarding(&self, step: Option<&Rc<Cell<bool>>>) -> Self {
        let mut steps = self.steps.clone();
        steps.extend(step.cloned());
        Self {
            errors: Vec::new(),
            first: self.first,
            steps,
        }
    }

    /// A `for` loop's sink, whose first execution is `step` when there is one.
    fn looping(step: Option<&Rc<Cell<bool>>>) -> Self {
        Self::new(false).forwarding(step)
    }

    /// Yields `error`; a `next(...)` consumer stops the generator there.
    fn emit(&mut self, error: PyError) -> Flow {
        for step in &self.steps {
            step.set(true);
        }
        self.errors.push(error);
        if self.first { Err(Stop::Done) } else { Ok(()) }
    }

    /// `next(generator, None) is None` once the generator ran.
    fn nothing_yielded(self, flow: Flow) -> Result<bool, PythonString> {
        match flow {
            Err(Stop::Raise(text)) => Err(text),
            Ok(()) | Err(Stop::Done) => Ok(self.errors.is_empty()),
        }
    }

    /// `list(generator)` once the generator ran.
    fn into_errors(self, flow: Flow) -> Result<Vec<PyError>, PythonString> {
        match flow {
            Err(Stop::Raise(text)) => Err(text),
            Ok(()) | Err(Stop::Done) => Ok(self.errors),
        }
    }
}

/// The `path` and `schema_path` steps `Validator.descend` prepends to every error.
#[derive(Default)]
struct Place {
    path: Option<PathSegment>,
    schema_path: Option<PathSegment>,
}

impl Place {
    fn path(path: PathSegment) -> Self {
        Self {
            path: Some(path),
            schema_path: None,
        }
    }

    fn schema_path(schema_path: PathSegment) -> Self {
        Self {
            path: None,
            schema_path: Some(schema_path),
        }
    }

    fn both(path: PathSegment, schema_path: PathSegment) -> Self {
        Self {
            path: Some(path),
            schema_path: Some(schema_path),
        }
    }
}

/// The ABCs `jsonschema` checks instances against with `isinstance`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Abc {
    /// `numbers.Number`, of `is_number`.
    Number,
    /// `collections.abc.Sequence`, of `_utils.equal`.
    Sequence,
    /// `collections.abc.Mapping`, of `_utils.equal`.
    Mapping,
}

/// What one `jsonschema.validate` call spends the recursion limit with: the model of where the limit sits, the
/// `(type, ABC)` pairs whose `isinstance` caches are already filled, and the patterns in the `re` cache. One
/// budget goes from `check_schema` into the validation, as the caches persist within one Python process. It also
/// carries the process's hash secret, which is the one `PYTHONHASHSEED=0` gives in [`Budget::default`].
#[derive(Debug, Default)]
struct Budget {
    model: RecursionModel,
    warm: RefCell<HashSet<(&'static str, Abc)>>,
    patterns: RefCell<crate::python_re_cache::ReCache>,
    secret: HashSecret,
    /// Set when an exception was raised whose `str()` raises `RecursionError` in the bridge's handler. The
    /// exception text alone cannot say so, since a schema's own strings reach exception texts.
    escaped: Cell<bool>,
}

impl Budget {
    fn new(model: RecursionModel, patterns: ReCache, secret: HashSecret) -> Self {
        Self {
            model,
            patterns: RefCell::new(patterns),
            secret,
            ..Self::default()
        }
    }

    /// The outcome of the exception `text` that stopped the validation: [`Outcome::Escaped`] when its `str()`
    /// raises, which clears the mark, or [`Outcome::Raised`].
    fn raised(&self, text: PythonString) -> Outcome {
        if self.escaped.take() {
            Outcome::Escaped(RECURSION_ERROR.to_owned())
        } else {
            Outcome::Raised(text)
        }
    }
}

/// What one validation shares: the registry of the root validator, the recursion budget and depth, and whether
/// the validators carry a `format_checker` (`evolve` hands it to every validator it makes), which only
/// `check_schema` passes.
struct Engine<'s> {
    registry: Registry<'s>,
    budget: Budget,
    depth: Cell<usize>,
    format_checker: bool,
    /// The C recursion checks under way, besides those of [`Self::steps`].
    held: Cell<usize>,
    /// The first executions of a `FOR_ITER` under way, each holding [`RESUMED`] checks until an error passes
    /// through it.
    steps: RefCell<Vec<Rc<Cell<bool>>>>,
}

/// Whether `schema` names its dialect with `$schema`, which makes building a validator for it look the dialect up.
fn names_dialect(schema: &Value) -> bool {
    schema.as_object().is_some_and(|object| object.contains_key("$schema"))
}

/// The frames above the caller of `validator.evolve(schema=schema)` reaches: `evolve`, the `attrs` `__init__` and
/// `__attrs_post_init__`, deeper by the dialect lookup when the schema names one.
fn evolve_frames(schema: &Value) -> usize {
    if names_dialect(schema) { 8 } else { 3 }
}

/// `str()` of an error in the bridge's handler, which pretty-prints `schema`, then `instance`, then dedents `text`:
/// the text, or `None` when that passes the limit of `model`. A value nested deeper than the frames left for its
/// plain `repr` passes it without being printed. The first long line it splits looks up or compiles
/// [`WORDS`], and a non-empty margin makes `textwrap.dedent` (one frame above `__str__`) look up or compile its
/// `re.sub` pattern; each joins `patterns` as `re._compile` stores it, unless the limit passed first.
fn printed(
    model: &RecursionModel,
    patterns: &RefCell<ReCache>,
    schema: &Value,
    instance: &Value,
    text: impl FnOnce() -> Dedented,
) -> Option<String> {
    let handler = model.handler_frame();
    for value in [schema, instance] {
        if model.exceeded(handler + 5 + 2 * ops::repr_depth(value)) {
            return None;
        }
        let cold = !patterns.borrow().contains(WORDS);
        let reach = pretty_frames(value, cold);
        if let Some(split) = reach.split
            && !model.exceeded(handler + split)
        {
            let mut patterns = patterns.borrow_mut();
            if cold {
                patterns.compiled(WORDS);
            } else {
                patterns.found(WORDS);
            }
        }
        if model.exceeded(handler + reach.peak) {
            return None;
        }
    }
    let dedented = text();
    if let Some(pattern) = dedented.pattern() {
        let sub = handler + 2;
        // A cached lookup reaches `sub + 1`, which printing a value (at least 7 frames below `__str__`) already passed.
        let pattern = PythonString::from(pattern);
        if patterns.borrow().contains(&pattern) {
            patterns.borrow_mut().found(&pattern);
        } else {
            if crate::python_re::compiles(&pattern, model.room(sub)) != Ok(true) {
                return None;
            }
            patterns.borrow_mut().compiled(&pattern);
        }
    }
    Some(dedented.text)
}

/// Frames charged to the depth, or C recursion checks to those held, until it drops.
struct Frames<'e> {
    depth: &'e Cell<usize>,
    cost: usize,
}

impl Drop for Frames<'_> {
    fn drop(&mut self) {
        self.depth.set(self.depth.get() - self.cost);
    }
}

/// The first execution of a `FOR_ITER` under way, if this one is, until it drops.
struct Step<'e> {
    steps: &'e RefCell<Vec<Rc<Cell<bool>>>>,
    step: Option<Rc<Cell<bool>>>,
}

impl Step<'_> {
    fn flag(&self) -> Option<&Rc<Cell<bool>>> {
        self.step.as_ref()
    }
}

impl Drop for Step<'_> {
    fn drop(&mut self) {
        if self.step.is_some() {
            self.steps.borrow_mut().pop();
        }
    }
}

/// A validator instance: its class (the draft), schema, resolver and `_validators`.
struct Validator<'v> {
    draft: Draft,
    schema: &'v Value,
    resolver: Resolver,
    validators: Vec<(&'v str, &'v Value, Keyword)>,
}

/// The class's `applicable_validators(schema)`.
fn applicable(draft: Draft, schema: &Value) -> Result<Vec<(&str, &Value)>, String> {
    if draft.ignores_ref_siblings()
        && let Some(reference) = ops::get_some(schema, "$ref")?
    {
        return Ok(vec![("$ref", reference)]);
    }
    Ok(ops::items(schema)?
        .iter()
        .filter_map(|(name, value)| Some((name.as_str()?, value)))
        .collect())
}

/// `TypeChecker.is_type(instance, kind)` of `draft`, whose `UndefinedTypeCheck` is `unknown`.
fn checks_type(draft: Draft, instance: &Value, kind: &Value, unknown: impl FnOnce() -> String) -> Result<bool, String> {
    if let Some(error) = ops::unhashable(kind) {
        return Err(error);
    }
    match kind {
        // A type name holding a lone surrogate names no type, as any other unknown name.
        Value::String(name) => name
            .as_str()
            .and_then(|name| draft.type_check(instance, name))
            .ok_or_else(unknown),
        _ => Err(unknown()),
    }
}

impl<'v> Validator<'v> {
    /// `__attrs_post_init__` once the resolver is known.
    fn new(draft: Draft, schema: &'v Value, resolver: Resolver) -> Result<Self, String> {
        let validators = if schema.is_boolean() {
            Vec::new()
        } else {
            applicable(draft, schema)?
                .into_iter()
                .filter_map(|(name, value)| Some((name, value, draft.keyword(name)?)))
                .collect()
        };
        Ok(Self {
            draft,
            schema,
            resolver,
            validators,
        })
    }

    /// `self.evolve(schema=schema, _resolver=resolver)`.
    fn evolve<'w>(&self, schema: &'w Value, resolver: Resolver) -> Result<Validator<'w>, PythonString> {
        Ok(Validator::new(
            drafts::validator_for(schema, self.draft)?,
            schema,
            resolver,
        )?)
    }
}

impl Engine<'_> {
    /// Raises `RecursionError` when a frame `cost` frames above the current one passes the limit.
    fn peek(&self, cost: usize) -> Result<(), String> {
        if self.budget.model.exceeded(self.depth.get() + cost) {
            return Err(RECURSION_ERROR.to_owned());
        }
        Ok(())
    }

    /// Pushes `cost` frames until the returned guard drops, raising `RecursionError` past the limit.
    fn enter(&self, cost: usize) -> Result<Frames<'_>, String> {
        self.peek(cost)?;
        self.depth.set(self.depth.get() + cost);
        Ok(Frames {
            depth: &self.depth,
            cost,
        })
    }

    /// Whether the process executes the instruction at `site` for the first time, which it now did.
    fn first(&self, site: Site) -> bool {
        let mut patterns = self.budget.patterns.borrow_mut();
        let first = patterns.executed & site.bit() == 0;
        patterns.executed |= site.bit();
        first
    }

    /// Holds `checks` C recursion checks until the returned guard drops.
    fn hold(&self, checks: usize) -> Frames<'_> {
        self.held.set(self.held.get() + checks);
        Frames {
            depth: &self.held,
            cost: checks,
        }
    }

    /// [`Self::hold`] around resuming a generator through a fast builtin call at `site`: [`RESUMED`], and
    /// [`BUILTIN_CALL`] more the first time.
    fn hold_call(&self, site: Site) -> Frames<'_> {
        let call = if self.first(site) { BUILTIN_CALL } else { 0 };
        self.hold(RESUMED + call)
    }

    /// The `FOR_ITER` at `site` resumes a generator: the first execution in the process holds [`RESUMED`] checks
    /// until the generator yields or returns, which the returned step marks.
    fn step(&self, site: Site) -> Step<'_> {
        let step = self.first(site).then(|| Rc::new(Cell::new(false)));
        if let Some(step) = &step {
            self.steps.borrow_mut().push(Rc::clone(step));
        }
        Step {
            steps: &self.steps,
            step,
        }
    }

    /// The C recursion checks under way.
    fn c_held(&self) -> usize {
        let resuming = self.steps.borrow().iter().filter(|step| !step.get()).count();
        self.held.get() + RESUMED * resuming
    }

    /// `{value!r}`.
    fn shown(&self, value: &Value) -> Result<String, String> {
        ops::shown(value, self.c_held())
    }

    /// `repr(value)`.
    fn repr_called(&self, value: &Value) -> Result<String, String> {
        ops::repr_called(value, self.c_held())
    }

    /// `", ".join(repr(each) for each in values)`.
    fn join_reprs<'v>(&self, values: impl IntoIterator<Item = &'v Value>) -> Result<String, String> {
        ops::join_reprs(values, self.c_held())
    }

    /// `jsonschema._utils.extras_msg`.
    fn extras_msg<'v>(&self, extras: impl IntoIterator<Item = &'v Value>) -> Result<(String, &'static str), String> {
        ops::extras_msg(extras, self.c_held())
    }

    /// The frames of `isinstance(value, abc)` for a value of the type `type_name`: `ABCMeta.__instancecheck__`
    /// alone once the pair is cached, the whole subclass check the first time in a cold process. A check that
    /// overflows caches nothing, so the next one walks again.
    fn abc(&self, type_name: &'static str, abc: Abc) -> Result<(), String> {
        let pair = (type_name, abc);
        if self.budget.model.cache() == CacheState::Cold && !self.budget.warm.borrow().contains(&pair) {
            self.peek(COLD_ABC_FRAMES)?;
            self.budget.warm.borrow_mut().insert(pair);
            return Ok(());
        }
        self.peek(1)
    }

    /// The frames of `TypeChecker.is_type(instance, kind)` below `Validator.is_type`: the `pmap` lookup of the
    /// check, then `is_number` with its `isinstance(instance, numbers.Number)` or the `is_integer` of drafts 6 and
    /// later, one frame deeper than the other checks.
    fn charge_type(&self, draft: Draft, instance: &Value, kind: Option<&str>) -> Result<(), String> {
        match kind {
            Some("number") if !instance.is_boolean() => {
                self.peek(3)?;
                let _frames = self.enter(3)?;
                self.abc(type_name(instance), Abc::Number)
            }
            Some("integer") if !matches!(draft, Draft::Draft3 | Draft::Draft4) => self.peek(4),
            _ => self.peek(3),
        }
    }

    /// `validator.is_type(instance, kind)` of `v`, whose unknown types raise `UnknownType`.
    fn is_type(&self, v: &Validator<'_>, instance: &Value, kind: &Value) -> Result<bool, String> {
        self.charge_type(v.draft, instance, kind.as_str())?;
        checks_type(v.draft, instance, kind, || {
            let text = || unknown_type_text(&repr(kind), v.schema, instance);
            printed(&self.budget.model, &self.budget.patterns, v.schema, instance, text).unwrap_or_else(|| {
                self.budget.escaped.set(true);
                RECURSION_ERROR.to_owned()
            })
        })
    }

    /// `validator.is_type(instance, kind)` of a type every draft's checker knows, as the keywords check their
    /// instance with.
    fn is_kind(&self, v: &Validator<'_>, instance: &Value, kind: &str) -> Result<bool, String> {
        self.charge_type(v.draft, instance, Some(kind))?;
        Ok(v.draft.type_check(instance, kind).unwrap_or(false))
    }

    /// `validator.evolve(schema=schema, _resolver=resolver)`.
    fn evolve_with<'w>(
        &self,
        v: &Validator<'_>,
        schema: &'w Value,
        resolver: Resolver,
    ) -> Result<Validator<'w>, PythonString> {
        self.peek(evolve_frames(schema))?;
        v.evolve(schema, resolver)
    }

    /// `validator.evolve(schema=schema)`.
    fn evolve<'w>(&self, v: &Validator<'_>, schema: &'w Value) -> Result<Validator<'w>, PythonString> {
        self.evolve_with(v, schema, v.resolver.clone())
    }

    /// The frames above `_validate_reference` (or a `find_evaluated_*` helper) that `resolver.lookup(reference)`
    /// reaches. `check_schema` looks up within the metaschemas: the `#meta` dynamic anchor, a JSON pointer or a
    /// whole resource. Otherwise a pointer, the root, or an anchor, which crawls the resource the first time.
    fn lookup_frames(&self, v: &Validator<'_>, reference: &Value) -> usize {
        let Value::String(reference) = reference else {
            return 1;
        };
        // `#` and `/` are ASCII, so they are whole code points of the encoding wherever its bytes hold them.
        let reference = reference.as_bytes();
        if self.format_checker {
            if reference == b"#meta" {
                return 7;
            }
            let fragment = reference
                .iter()
                .position(|byte| *byte == b'#')
                .and_then(|at| reference.get(at + 1..))
                .unwrap_or_default();
            return if fragment.starts_with(b"/") { 6 } else { 5 };
        }
        let fragment = reference.strip_prefix(b"#").unwrap_or(reference);
        if fragment.starts_with(b"/") {
            6
        } else if fragment.is_empty() || v.resolver.crawled() {
            5
        } else {
            8
        }
    }

    /// The frames above the caller that `resolver.lookup(...)` of a `$recursiveRef` reaches, deeper outside the
    /// metaschemas.
    fn recursive_lookup_frames(&self) -> usize {
        if self.format_checker { 5 } else { 6 }
    }

    /// The frame budget below the `re.search` or `re.compile` frame `up` frames above the current one, or `None`
    /// for a pattern in the `re` cache, which compiles nothing: only `_compile` is charged, and the lookup is done.
    fn regex_budget(&self, pattern: &PythonString, up: usize) -> Result<Option<usize>, String> {
        if !self.budget.patterns.borrow().contains(pattern) {
            return Ok(Some(self.budget.model.room(self.depth.get() + up)));
        }
        self.peek(up + 1)?;
        self.budget.patterns.borrow_mut().found(pattern);
        Ok(None)
    }

    /// `re.search(pattern, text)` from a frame `up` frames above the current one; a pattern it compiles joins the
    /// `re` cache.
    fn search_str(&self, pattern: &PythonString, text: &PythonString, up: usize) -> Result<bool, PythonString> {
        let budget = self.regex_budget(pattern, up)?;
        let found = crate::python_re::search(pattern, text, budget.unwrap_or(usize::MAX))?;
        if budget.is_some() {
            self.budget.patterns.borrow_mut().compiled(pattern);
        }
        Ok(found)
    }

    /// [`Self::search_str`] of a pattern that may be any value, from a frame one above the current one:
    /// `re._compile` raises for one that is not a string, after the cache lookup that hashes it. The frames it
    /// reaches first (`re.search` and `_compile`, and `_compiler.isstring` for a hashable pattern) are no deeper
    /// than the `validator.is_type` both callers ran from the current frame just before, so no `RecursionError`
    /// can come from them.
    fn search(&self, pattern: &Value, text: &PythonString) -> Result<bool, PythonString> {
        match pattern {
            Value::String(pattern) => self.search_str(pattern, text, 1),
            _ => Err(ops::unhashable(pattern)
                .unwrap_or_else(|| "first argument must be string or compiled pattern".to_owned())
                .into()),
        }
    }

    /// `re.compile(pattern)` inside `try: ... except re.error`, from a frame `up` frames above the current one: whether
    /// it compiles; a compiled pattern stays in the `re` cache.
    fn compiles(&self, pattern: &PythonString, up: usize) -> Result<bool, PythonString> {
        let budget = self.regex_budget(pattern, up)?;
        let compiled = crate::python_re::compiles(pattern, budget.unwrap_or(usize::MAX))?;
        if compiled && budget.is_some() {
            self.budget.patterns.borrow_mut().compiled(pattern);
        }
        Ok(compiled)
    }

    /// `Validator.iter_errors(instance)`.
    fn iter_errors(&self, v: &Validator<'_>, instance: &Value, sink: &mut Sink) -> Flow {
        let _frames = self.enter(1)?;
        match v.schema {
            Value::Bool(true) => return Ok(()),
            Value::Bool(false) => return sink.emit(PyError::false_schema(instance, v.schema, self.c_held())?),
            _ => {}
        }
        for &entry in &v.validators {
            self.apply(entry, v, instance, &Place::default(), Site::IterErrors, sink)?;
        }
        Ok(())
    }

    /// `Validator.descend(instance, schema, path, schema_path, resolver)`.
    fn descend(
        &self,
        v: &Validator<'_>,
        instance: &Value,
        schema: &Value,
        place: Place,
        resolver: Option<Resolver>,
        sink: &mut Sink,
    ) -> Flow {
        let _frames = self.enter(1)?;
        match schema {
            Value::Bool(true) => return Ok(()),
            Value::Bool(false) => return sink.emit(PyError::false_schema(instance, schema, self.c_held())?),
            _ => {}
        }
        // `in_subresource` and `evolve`, measured one frame deeper under drafts before 2019-09, and deeper still
        // with the dialect lookup of a schema naming its own.
        self.peek(if names_dialect(schema) {
            8
        } else if matches!(v.draft, Draft::Draft201909 | Draft::Draft202012) {
            3
        } else {
            4
        })?;
        let resolver = match resolver {
            Some(resolver) => resolver,
            None => v.resolver.entered(schema, v.draft)?,
        };
        let evolved = v.evolve(schema, resolver)?;
        for (name, value) in applicable(v.draft, schema)? {
            if let Some(keyword) = evolved.draft.keyword(name) {
                self.apply((name, value, keyword), &evolved, instance, &place, Site::Descend, sink)?;
            }
        }
        Ok(())
    }

    /// Runs the keyword function of `entry` (the keyword, its value and its function) for `v`, and yields its
    /// errors as `iter_errors` and `descend` do with their `for error in errors` at `site`.
    fn apply(
        &self,
        (name, value, keyword): (&str, &Value, Keyword),
        v: &Validator<'_>,
        instance: &Value,
        place: &Place,
        site: Site,
        sink: &mut Sink,
    ) -> Flow {
        let step = self.step(site);
        let _frames = self.enter(1)?;
        let mut produced = sink.forwarding(step.flag());
        let flow = self.keyword(keyword, v, value, instance, &mut produced);
        for mut error in produced.errors {
            error.set(name, value, instance, v.schema, v.draft);
            if name != "if" && name != "$ref" {
                error.schema_path.push_front(PathSegment::Key(name.into()));
            }
            if let Some(path) = &place.path {
                error.path.push_front(path.clone());
            }
            if let Some(schema_path) = &place.schema_path {
                error.schema_path.push_front(schema_path.clone());
            }
            sink.emit(error)?;
        }
        flow
    }

    /// `list(validator.descend(instance, schema, path, schema_path))`.
    fn descend_all(
        &self,
        v: &Validator<'_>,
        instance: &Value,
        schema: &Value,
        place: Place,
    ) -> Result<Vec<PyError>, PythonString> {
        let _resumed = self.hold(RESUMED);
        let mut sink = Sink::new(false);
        let flow = self.descend(v, instance, schema, place, None, &mut sink);
        sink.into_errors(flow)
    }

    /// `for error in validator.descend(instance, schema, path, schema_path)` at `site`, a loop that only collects
    /// the errors.
    fn descend_looping(
        &self,
        v: &Validator<'_>,
        instance: &Value,
        schema: &Value,
        place: Place,
        site: Site,
    ) -> Result<Vec<PyError>, PythonString> {
        let step = self.step(site);
        let mut sink = Sink::looping(step.flag());
        let flow = self.descend(v, instance, schema, place, None, &mut sink);
        drop(step);
        sink.into_errors(flow)
    }

    /// `next(validator.descend(instance, schema), None) is None`, the `next` call at `site`.
    fn descend_valid(
        &self,
        v: &Validator<'_>,
        instance: &Value,
        schema: &Value,
        site: Site,
    ) -> Result<bool, PythonString> {
        let _resumed = self.hold_call(site);
        let mut sink = Sink::new(true);
        let flow = self.descend(v, instance, schema, Place::default(), None, &mut sink);
        sink.nothing_yielded(flow)
    }

    /// `validator.is_valid(instance)`.
    fn is_valid(&self, v: &Validator<'_>, instance: &Value) -> Result<bool, PythonString> {
        let _frames = self.enter(1)?;
        let _resumed = self.hold_call(Site::IsValid);
        let mut sink = Sink::new(true);
        let flow = self.iter_errors(v, instance, &mut sink);
        sink.nothing_yielded(flow)
    }
}

/// The `relevance` key of an error: `(-len(path), path, validator not in WEAK_MATCHES, False, not
/// error._matches_type())`.
struct Key<'e> {
    path: &'e VecDeque<PathSegment>,
    strong: bool,
    mismatch: bool,
}

/// Python's `None`, for an error whose instance is still unset.
static NONE: Value = Value::Null;

/// `error._matches_type()`.
fn matches_type(error: &PyError) -> Result<bool, String> {
    let Some(Value::Object(schema)) = &error.schema else {
        return Ok(false);
    };
    let Some(expected) = schema.get("type") else {
        return Ok(false);
    };
    let checker = error.type_checker.unwrap_or(Draft::Draft202012);
    let instance = error.instance.as_ref().unwrap_or(&NONE);
    let unknown = |kind: &Value| format!("Type {} is unknown to this type checker", repr(kind));
    if expected.is_string() {
        return checks_type(checker, instance, expected, || unknown(expected));
    }
    for kind in ops::iterate(expected)? {
        let kind: &Value = &kind;
        if checks_type(checker, instance, kind, || unknown(kind))? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `relevance(error)`.
fn relevance(error: &PyError) -> Result<Key<'_>, String> {
    let weak = matches!(&error.validator, Some(Some(name)) if name == "anyOf" || name == "oneOf");
    Ok(Key {
        path: &error.path,
        strong: !weak,
        mismatch: !matches_type(error)?,
    })
}

/// `left == right` of two relevance keys.
fn same(left: &Key<'_>, right: &Key<'_>) -> bool {
    left.path == right.path && left.strong == right.strong && left.mismatch == right.mismatch
}

/// `left OP right` of two relevance keys, `OP` being `<` or `>`.
fn exceeds(left: &Key<'_>, op: Op, right: &Key<'_>) -> Result<bool, String> {
    let greater = op == Op::Gt;
    if left.path.len() != right.path.len() {
        return Ok((left.path.len() < right.path.len()) == greater);
    }
    if let Some((a, b)) = left.path.iter().zip(right.path).find(|(a, b)| a != b) {
        return match (a, b) {
            (PathSegment::Key(a), PathSegment::Key(b)) => Ok((a > b) == greater),
            (PathSegment::Index(a), PathSegment::Index(b)) => Ok((a > b) == greater),
            (PathSegment::Key(_), PathSegment::Index(_)) => Err(ops::unorderable(op, "str", "int")),
            (PathSegment::Index(_), PathSegment::Key(_)) => Err(ops::unorderable(op, "int", "str")),
        };
    }
    for (a, b) in [(left.strong, right.strong), (left.mismatch, right.mismatch)] {
        if a != b {
            return Ok(a == greater);
        }
    }
    Ok(false)
}

/// `max(errors, key=relevance, default=None)`.
fn most_relevant(errors: &[PyError]) -> Result<Option<&PyError>, String> {
    let mut best: Option<(&PyError, Key<'_>)> = None;
    for error in errors {
        let key = relevance(error)?;
        let replace = match &best {
            None => true,
            Some((_, top)) => exceeds(&key, Op::Gt, top)?,
        };
        if replace {
            best = Some((error, key));
        }
    }
    Ok(best.map(|(error, _)| error))
}

/// A `(key, order, error)` entry of `heapq.nsmallest`.
struct Entry<'e> {
    key: Key<'e>,
    order: usize,
    error: &'e PyError,
}

/// `left < right` of two `heapq.nsmallest` entries.
fn precedes(left: &Entry<'_>, right: &Entry<'_>) -> Result<bool, String> {
    if same(&left.key, &right.key) {
        return Ok(left.order < right.order);
    }
    exceeds(&left.key, Op::Lt, &right.key)
}

/// One `(key(error), order, error)` entry of `heapq.nsmallest`.
fn entry(order: usize, error: &PyError) -> Result<Entry<'_>, String> {
    Ok(Entry {
        key: relevance(error)?,
        order,
        error,
    })
}

/// `heapq.nsmallest(2, context, key=relevance)`, with the keys of the entries it returns.
fn two_smallest(context: &[PyError]) -> Result<Vec<Entry<'_>>, String> {
    let mut errors = context.iter().enumerate();
    let mut entries = Vec::with_capacity(2);
    if context.len() <= 2 {
        // `sorted(context, key=relevance)[:2]`: every key first, then one `<` for two entries.
        for (order, error) in errors {
            entries.push(entry(order, error)?);
        }
        if let [first, second] = entries.as_slice()
            && exceeds(&second.key, Op::Lt, &first.key)?
        {
            entries.swap(0, 1);
        }
        return Ok(entries);
    }
    for (order, error) in errors.by_ref().take(2) {
        entries.push(entry(order, error)?);
    }
    // `_heapify_max(result)`.
    entries.swap(0, 1);
    sift_down_max(&mut entries)?;
    let mut order = 2;
    for (_, error) in errors {
        let key = relevance(error)?;
        if exceeds(&key, Op::Lt, &entries[0].key)? {
            // `_heapreplace_max(result, (key, order, error))`.
            entries[0] = Entry { key, order, error };
            entries.swap(0, 1);
            sift_down_max(&mut entries)?;
            order += 1;
        }
    }
    // `result.sort()`.
    if let [first, second] = entries.as_slice()
        && precedes(second, first)?
    {
        entries.swap(0, 1);
    }
    Ok(entries)
}

/// `_siftdown_max(heap, 0, 1)` of a two-entry heap: the parent moves down when it is smaller than the child.
fn sift_down_max(entries: &mut [Entry<'_>]) -> Result<(), String> {
    if let [parent, child] = &*entries
        && precedes(parent, child)?
    {
        entries.swap(0, 1);
    }
    Ok(())
}

/// `jsonschema.exceptions.best_match` over the errors `iter_errors` yielded before `raised` stopped it.
fn select(errors: &[PyError], raised: Option<Outcome>) -> Outcome {
    let best = match most_relevant(errors) {
        Ok(best) => best,
        Err(text) => return Outcome::Raised(text.into()),
    };
    if let Some(outcome) = raised {
        return outcome;
    }
    let Some(mut best) = best else {
        return Outcome::Valid;
    };
    while !best.context.is_empty() {
        let smallest = match two_smallest(&best.context) {
            Ok(smallest) => smallest,
            Err(text) => return Outcome::Raised(text.into()),
        };
        let tie = matches!(smallest.as_slice(), [first, second] if same(&first.key, &second.key));
        match smallest.first() {
            Some(first) if !tie => best = first.error,
            _ => break,
        }
    }
    Outcome::Invalid(best.message.clone())
}

#[cfg(test)]
#[path = "python_jsonschema_tests.rs"]
mod tests;
