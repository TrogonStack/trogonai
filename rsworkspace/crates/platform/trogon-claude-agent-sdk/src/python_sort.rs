//! Exact port of jsonschema 4.26 `_utils.uniq` (the `uniqueItems` keyword) on CPython 3.12.
//!
//! ```python
//! def uniq(container):
//!     try:
//!         sort = sorted(unbool(i) for i in container)
//!         sliced = itertools.islice(sort, 1, None)
//!         for i, j in zip(sort, sliced):
//!             if equal(i, j):
//!                 return False
//!     except (NotImplementedError, TypeError):
//!         seen = []
//!         for e in container:
//!             e = unbool(e)
//!             for i in seen:
//!                 if equal(i, e):
//!                     return False
//!             seen.append(e)
//!     return True
//! ```
//!
//! Whether the `TypeError` fallback runs depends on which pairs `list.sort` actually compares, so
//! the sort is a line-for-line port of CPython 3.12 `list.sort` (`count_run`, `binarysort`,
//! `merge_compute_minrun`, the powersort run stack, `gallop_left`/`gallop_right`, `merge_lo`,
//! `merge_hi` with the adaptive `min_gallop`), working on a permutation of indices so the
//! comparison sequence is identical to CPython's. The per-type fast paths of `list.sort`
//! (`unsafe_long_compare`, `unsafe_latin_compare`, ...) compute the same results as the generic
//! `<`. What they change is where the C recursion checks of nested comparisons start, which
//! [`sorted_within`] models: `PyObject_RichCompare` checks once a level, so comparing lists
//! nested past the checks left raises `RecursionError`.
//!
//! The comparison hook records `(left, right)` original indices of every `<` call, including the
//! one that raises.

use crate::constants::MIN_GALLOP;
use std::cmp::Ordering;
use std::collections::HashMap;

use crate::{
    python_arith::{PyNumber, big_cmp_float, int_cmp, number},
    python_json_value::{ExactInteger, PythonJsonValue as Value},
    python_string::PythonString,
};

/// A Python value as jsonschema's `uniq` sees it (a decoded JSON document).
#[derive(Debug, Clone)]
pub(crate) enum PyValue {
    None,
    Bool(bool),
    Int(i128),
    /// An `int` outside the `i128` range.
    Big(ExactInteger),
    Float(f64),
    Str(PythonString),
    List(Vec<PyValue>),
    Dict(Vec<(PythonString, PyValue)>),
    /// The `true` / `false` singleton `object()` that `unbool` substitutes for a top-level bool.
    Sentinel(bool),
}

/// Python's `TypeError` from a `<` between unorderable operands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TypeError;

/// What `sorted(keys)` raised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SortError {
    /// `TypeError`: two keys do not order.
    Type,
    /// `RecursionError`: a comparison nested past the C recursion checks left.
    Recursion,
}

/// A container [`PyValue::from_json`] is still building: the members left to convert and those converted, with
/// the key of the member under conversion for a dict.
enum Open<'v> {
    List(std::slice::Iter<'v, Value>, Vec<PyValue>),
    Dict(
        indexmap::map::Iter<'v, PythonString, Value>,
        Vec<(PythonString, PyValue)>,
        PythonString,
    ),
}

impl<'v> Open<'v> {
    /// The next member to convert.
    fn next_member(&mut self) -> Option<&'v Value> {
        match self {
            Self::List(items, _) => items.next(),
            Self::Dict(entries, _, key) => entries.next().map(|(name, value)| {
                name.clone_into(key);
                value
            }),
        }
    }

    /// Adds the member just converted.
    fn push(&mut self, value: PyValue) {
        match self {
            Self::List(_, built) => built.push(value),
            Self::Dict(_, built, key) => built.push((std::mem::take(key), value)),
        }
    }

    fn close(self) -> PyValue {
        match self {
            Self::List(_, built) => PyValue::List(built),
            Self::Dict(_, built, _) => PyValue::Dict(built),
        }
    }
}

impl PyValue {
    /// The value `json.loads` makes of `value`, built without recursing however deep it nests.
    pub(crate) fn from_json(value: &Value) -> Self {
        let mut open: Vec<Open<'_>> = Vec::new();
        let mut next = Some(value);
        let mut built = None;
        loop {
            match next.take() {
                Some(Value::Array(items)) => open.push(Open::List(items.iter(), Vec::with_capacity(items.len()))),
                Some(Value::Object(map)) => {
                    open.push(Open::Dict(
                        map.iter(),
                        Vec::with_capacity(map.len()),
                        PythonString::new(),
                    ));
                }
                Some(scalar) => built = Some(Self::scalar(scalar)),
                None => {}
            }
            let Some(top) = open.last_mut() else {
                return built.unwrap_or(Self::None);
            };
            if let Some(member) = built.take() {
                top.push(member);
            }
            next = top.next_member();
            if next.is_none() {
                built = open.pop().map(Open::close);
            }
        }
    }

    /// The value of a JSON scalar; a container stands for nothing here.
    fn scalar(value: &Value) -> Self {
        match value {
            Value::Bool(b) => Self::Bool(*b),
            Value::Number(n) => match number(n) {
                PyNumber::Int(i) => Self::Int(i),
                PyNumber::Big(big) => Self::Big(big),
                PyNumber::Float(f) => Self::Float(f),
            },
            Value::String(s) => Self::Str(s.clone()),
            Value::Null | Value::Array(_) | Value::Object(_) => Self::None,
        }
    }
}

impl Drop for PyValue {
    /// Takes the containers apart one by one, so a value nested past the native stack drops without recursing.
    fn drop(&mut self) {
        let mut pending = Vec::new();
        self.take_members(&mut pending);
        while let Some(mut value) = pending.pop() {
            value.take_members(&mut pending);
        }
    }
}

impl PyValue {
    /// Moves the members of a container to `pending`, leaving it empty.
    fn take_members(&mut self, pending: &mut Vec<Self>) {
        match self {
            Self::List(items) => pending.append(items),
            Self::Dict(entries) => pending.extend(std::mem::take(entries).into_iter().map(|(_, member)| member)),
            _ => {}
        }
    }
}

/// `one is two` for values `json.loads` decoded separately: CPython shares the cached small ints, the empty string
/// and the one-character Latin-1 strings, besides `None`, the booleans and the `unbool` sentinels.
pub(crate) fn identical(one: &PyValue, two: &PyValue) -> bool {
    match (one, two) {
        (PyValue::Int(left), PyValue::Int(right)) => left == right && (-5..=256).contains(left),
        (PyValue::Str(left), PyValue::Str(right)) => {
            left == right && left.code_points().nth(1).is_none() && left.code_points().all(|c| c.as_u32() < 256)
        }
        (PyValue::None, PyValue::None) => true,
        (PyValue::Bool(left), PyValue::Bool(right)) | (PyValue::Sentinel(left), PyValue::Sentinel(right)) => {
            left == right
        }
        _ => false,
    }
}

/// jsonschema `unbool`: a top-level `True` / `False` becomes a unique sentinel object.
pub(crate) fn unbool(value: PyValue) -> PyValue {
    match value {
        PyValue::Bool(b) => PyValue::Sentinel(b),
        other => other,
    }
}

#[derive(Clone, Copy)]
enum Num<'a> {
    Int(i128),
    Big(&'a ExactInteger),
    Float(f64),
}

fn as_num(value: &PyValue) -> Option<Num<'_>> {
    match value {
        PyValue::Bool(b) => Some(Num::Int(i128::from(*b))),
        PyValue::Int(i) => Some(Num::Int(*i)),
        PyValue::Big(big) => Some(Num::Big(big)),
        PyValue::Float(f) => Some(Num::Float(*f)),
        _ => None,
    }
}

/// `2**127` as an `f64`: the first float above every `i128`.
fn two_pow_127() -> f64 {
    (1_u128 << 127) as f64
}

fn int_eq_float(i: i128, f: f64) -> bool {
    f.is_finite() && f.fract() == 0.0 && (-two_pow_127()..two_pow_127()).contains(&f) && f as i128 == i
}

/// Python `int < float`, exact.
fn int_lt_float(i: i128, f: f64) -> bool {
    if f.is_nan() || f == f64::NEG_INFINITY {
        return false;
    }
    if f >= two_pow_127() {
        return true;
    }
    if f < -two_pow_127() {
        return false;
    }
    i < f.ceil() as i128
}

/// Python `float < int`, exact.
fn float_lt_int(f: f64, i: i128) -> bool {
    if f.is_nan() || f >= two_pow_127() {
        return false;
    }
    if f < -two_pow_127() {
        return true;
    }
    (f.floor() as i128) < i
}

fn num_eq(a: Num<'_>, b: Num<'_>) -> bool {
    match (a, b) {
        (Num::Int(x), Num::Int(y)) => x == y,
        (Num::Float(x), Num::Float(y)) => x == y,
        (Num::Int(x), Num::Float(y)) | (Num::Float(y), Num::Int(x)) => int_eq_float(x, y),
        (Num::Big(x), Num::Big(y)) => x == y,
        (Num::Big(_), Num::Int(_)) | (Num::Int(_), Num::Big(_)) => false,
        (Num::Big(x), Num::Float(y)) | (Num::Float(y), Num::Big(x)) => big_cmp_float(x, y) == Some(Ordering::Equal),
    }
}

fn num_lt(a: Num<'_>, b: Num<'_>) -> bool {
    match (a, b) {
        (Num::Int(x), Num::Int(y)) => x < y,
        (Num::Float(x), Num::Float(y)) => x < y,
        (Num::Int(x), Num::Float(y)) => int_lt_float(x, y),
        (Num::Float(x), Num::Int(y)) => float_lt_int(x, y),
        (Num::Big(x), Num::Big(y)) => int_cmp(x, y) == Ordering::Less,
        // Every `int` outside the `i128` range lies beyond every `i128`.
        (Num::Big(x), Num::Int(_)) => x.is_negative(),
        (Num::Int(_), Num::Big(y)) => !y.is_negative(),
        (Num::Big(x), Num::Float(y)) => big_cmp_float(x, y) == Some(Ordering::Less),
        (Num::Float(x), Num::Big(y)) => big_cmp_float(y, x) == Some(Ordering::Greater),
    }
}

/// Whether every pair of `pending` holds two equal values under `shallow`, which answers a pair of scalars or
/// pushes the members a pair of containers holds: the walk of a nested `==` without recursing.
fn all_pairs<'a>(
    mut pending: Vec<(&'a PyValue, &'a PyValue)>,
    shallow: impl Fn(&'a PyValue, &'a PyValue) -> Option<bool>,
) -> bool {
    while let Some((one, two)) = pending.pop() {
        if let Some(equal) = shallow(one, two) {
            if !equal {
                return false;
            }
            continue;
        }
        match (one, two) {
            (PyValue::List(x), PyValue::List(y)) if x.len() == y.len() => pending.extend(x.iter().zip(y).rev()),
            (PyValue::Dict(x), PyValue::Dict(y)) if x.len() == y.len() => {
                for (key, value) in x.iter().rev() {
                    let Some((_, other)) = y.iter().find(|(other, _)| other == key) else {
                        return false;
                    };
                    pending.push((value, other));
                }
            }
            _ => return false,
        }
    }
    true
}

/// Python `==` (`bool` is an `int`; `1 == 1.0 == True`).
pub(crate) fn py_eq(a: &PyValue, b: &PyValue) -> bool {
    all_pairs(vec![(a, b)], scalar_eq)
}

/// Python `==` of two values unless both are lists or both dicts, whose members decide.
fn scalar_eq(a: &PyValue, b: &PyValue) -> Option<bool> {
    if let (Some(x), Some(y)) = (as_num(a), as_num(b)) {
        return Some(num_eq(x, y));
    }
    match (a, b) {
        (PyValue::List(_), PyValue::List(_)) | (PyValue::Dict(_), PyValue::Dict(_)) => None,
        (PyValue::None, PyValue::None) => Some(true),
        (PyValue::Str(x), PyValue::Str(y)) => Some(x == y),
        (PyValue::Sentinel(x), PyValue::Sentinel(y)) => Some(x == y),
        _ => Some(false),
    }
}

/// Python `<`: numbers, strings and lists order; every other pairing raises `TypeError`. Two lists
/// compare as CPython `list_richcompare` does: the first index whose items differ under `==` decides,
/// otherwise the lengths do.
pub(crate) fn py_lt(a: &PyValue, b: &PyValue) -> Result<bool, TypeError> {
    let (mut a, mut b) = (a, b);
    loop {
        if let (Some(x), Some(y)) = (as_num(a), as_num(b)) {
            return Ok(num_lt(x, y));
        }
        match (a, b) {
            (PyValue::Str(x), PyValue::Str(y)) => return Ok(x < y),
            (PyValue::List(x), PyValue::List(y)) => match x.iter().zip(y).find(|(p, q)| !py_eq(p, q)) {
                Some((p, q)) => (a, b) = (p, q),
                None => return Ok(x.len() < y.len()),
            },
            _ => return Err(TypeError),
        }
    }
}

/// jsonschema `equal`, with `unbool` applied to non-container operands.
pub(crate) fn equal(one: &PyValue, two: &PyValue) -> bool {
    all_pairs(vec![(one, two)], |one, two| match (one, two) {
        (PyValue::Str(_), _) | (_, PyValue::Str(_)) => Some(py_eq(one, two)),
        (PyValue::List(_), PyValue::List(_)) | (PyValue::Dict(_), PyValue::Dict(_)) => None,
        (PyValue::Bool(x) | PyValue::Sentinel(x), PyValue::Bool(y) | PyValue::Sentinel(y)) => Some(x == y),
        (PyValue::Bool(_) | PyValue::Sentinel(_), _) | (_, PyValue::Bool(_) | PyValue::Sentinel(_)) => Some(false),
        _ => Some(py_eq(one, two)),
    })
}

/// A rich comparison operator a sort's comparisons use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Rich {
    Eq,
    Lt,
}

/// A container comparison under way inside [`Compare`], at `depth` C recursion checks.
#[derive(Clone, Copy)]
enum Pending<'a> {
    /// `list_richcompare(a, b, op)`, the items before `index` equal.
    List {
        a: &'a [PyValue],
        b: &'a [PyValue],
        op: Rich,
        depth: usize,
        index: usize,
    },
    /// `dict_equal(a, b)`, the entries before `index` equal.
    Dict {
        a: &'a [(PythonString, PyValue)],
        b: &'a [(PythonString, PyValue)],
        depth: usize,
        index: usize,
    },
}

/// One comparison `list.sort` makes, as CPython 3.12 runs it with `room` C recursion checks left: each
/// `PyObject_RichCompare` checks once before it compares, and the comparisons it nests run under its check. The
/// containers under way wait on a stack, so no nesting recurses here.
struct Compare<'a> {
    room: usize,
    pending: Vec<Pending<'a>>,
    /// The index at which each list comparison under `==` that finished found its first unequal members, so a
    /// `<` of the same lists at the same depth goes straight to them: CPython repeats those `==` comparisons with
    /// the same answers, and repeating them here walks two deep lists once per level.
    unequal: HashMap<ListPair, usize>,
}

/// Two lists compared at a depth, by where their items live.
type ListPair = (*const PyValue, usize, *const PyValue, usize, usize);

/// The [`ListPair`] of `a` and `b` compared at `depth`.
fn list_pair(a: &[PyValue], b: &[PyValue], depth: usize) -> ListPair {
    (a.as_ptr(), a.len(), b.as_ptr(), b.len(), depth)
}

/// A step of a [`Compare`]: the answer of the comparison it ran, or `None` once it opened a container comparison.
type Answer = Option<Result<bool, SortError>>;

impl<'a> Compare<'a> {
    /// `PyObject_RichCompare(v, w, op)` under `depth` checks.
    fn rich(&mut self, v: &'a PyValue, w: &'a PyValue, op: Rich, depth: usize) -> Answer {
        if depth + 1 > self.room {
            return Some(Err(SortError::Recursion));
        }
        self.compare(v, w, op, depth + 1)
    }

    /// `PyObject_RichCompareBool(v, w, Py_EQ)` under `depth` checks: an object equals itself before any check.
    fn rich_eq(&mut self, v: &'a PyValue, w: &'a PyValue, depth: usize) -> Answer {
        if identical(v, w) {
            return Some(Ok(true));
        }
        self.rich(v, w, Rich::Eq, depth)
    }

    /// `do_richcompare(v, w, op)` once `depth` checks are under way: two lists, or two dicts under `==`, open a
    /// comparison of their members; `<` of two dicts, or of values of types that do not order, raises
    /// `TypeError`.
    fn compare(&mut self, v: &'a PyValue, w: &'a PyValue, op: Rich, depth: usize) -> Answer {
        match (v, w, op) {
            (PyValue::List(a), PyValue::List(b), _) => {
                if op == Rich::Eq && a.len() != b.len() {
                    return Some(Ok(false));
                }
                self.pending.push(Pending::List {
                    a,
                    b,
                    op,
                    depth,
                    index: 0,
                });
                None
            }
            (PyValue::Dict(a), PyValue::Dict(b), Rich::Eq) => {
                if a.len() != b.len() {
                    return Some(Ok(false));
                }
                self.pending.push(Pending::Dict { a, b, depth, index: 0 });
                None
            }
            (_, _, Rich::Eq) => Some(Ok(py_eq(v, w))),
            (_, _, Rich::Lt) => Some(py_lt(v, w).map_err(|TypeError| SortError::Type)),
        }
    }

    /// Records that the comparison under way waits on its member at `at`.
    fn wait_on(&mut self, at: usize) {
        if let Some(Pending::List { index, .. } | Pending::Dict { index, .. }) = self.pending.last_mut() {
            *index = at;
        }
    }

    /// Goes on with `top`, the comparison under way, once its member at `index` compared equal or not under `==`
    /// (`equal`), or from the start when it just opened.
    fn resume(&mut self, top: Pending<'a>, equal: Option<bool>) -> Answer {
        match top {
            Pending::List {
                a,
                b,
                op,
                depth,
                mut index,
            } => {
                if let Some(equal) = equal {
                    if !equal {
                        self.pending.pop();
                        return match op {
                            Rich::Eq => {
                                self.unequal.insert(list_pair(a, b, depth), index);
                                Some(Ok(false))
                            }
                            Rich::Lt => self.rich(&a[index], &b[index], Rich::Lt, depth),
                        };
                    }
                    index += 1;
                } else if op == Rich::Lt
                    && let Some(&at) = self.unequal.get(&list_pair(a, b, depth))
                {
                    self.pending.pop();
                    return self.rich(&a[at], &b[at], Rich::Lt, depth);
                }
                while let (Some(v), Some(w)) = (a.get(index), b.get(index)) {
                    if !identical(v, w) {
                        self.wait_on(index);
                        return self.rich_eq(v, w, depth);
                    }
                    index += 1;
                }
                self.pending.pop();
                Some(Ok(match op {
                    Rich::Eq => a.len() == b.len(),
                    Rich::Lt => a.len() < b.len(),
                }))
            }
            Pending::Dict { a, b, depth, mut index } => {
                if let Some(equal) = equal {
                    if !equal {
                        self.pending.pop();
                        return Some(Ok(false));
                    }
                    index += 1;
                }
                let Some((key, v)) = a.get(index) else {
                    self.pending.pop();
                    return Some(Ok(true));
                };
                let Some((_, w)) = b.iter().find(|(other, _)| other == key) else {
                    self.pending.pop();
                    return Some(Ok(false));
                };
                self.wait_on(index);
                self.rich_eq(v, w, depth)
            }
        }
    }

    /// `v < w` as `list.sort` compares two keys: through `PyObject_RichCompareBool` when `checked`, directly
    /// otherwise.
    fn lt(mut self, v: &'a PyValue, w: &'a PyValue, checked: bool) -> Result<bool, SortError> {
        let mut answer = if checked {
            self.rich(v, w, Rich::Lt, 0)
        } else {
            self.compare(v, w, Rich::Lt, 0)
        };
        while let Some(&top) = self.pending.last() {
            let equal = answer.take().transpose()?;
            answer = self.resume(top, equal);
        }
        answer.unwrap_or(Ok(false))
    }
}

/// Whether `list.sort` compares `keys` through `PyObject_RichCompareBool`: unless every key has the same type
/// and that type's own comparison answers `<`, as for `list`, `int`, `float`, `str` and `bool`.
fn checked(keys: &[PyValue]) -> bool {
    let Some(first) = keys.first() else {
        return true;
    };
    // Both `Int` and `Big` are Python `int`.
    let kind = |key: &PyValue| match key {
        PyValue::Big(_) => std::mem::discriminant(&PyValue::Int(0)),
        _ => std::mem::discriminant(key),
    };
    !keys.iter().all(|key| kind(key) == kind(first))
        || !matches!(
            first,
            PyValue::List(_)
                | PyValue::Int(_)
                | PyValue::Big(_)
                | PyValue::Float(_)
                | PyValue::Str(_)
                | PyValue::Bool(_)
        )
}

/// jsonschema `uniq` over JSON array items.
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "the validator charges each comparison through its own uniq; the tests check this one against the recorded sorts"
    )
)]
pub(crate) fn uniq(items: &[Value]) -> bool {
    uniq_traced(items, None)
}

/// [`uniq`], optionally recording every `list.sort` comparison as `(left, right)` indices into
/// `items`.
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "the validator charges each comparison through its own uniq; the tests check this one against the recorded sorts"
    )
)]
pub(crate) fn uniq_traced(items: &[Value], trace: Option<&mut Vec<(usize, usize)>>) -> bool {
    let keys: Vec<PyValue> = items.iter().map(|item| unbool(PyValue::from_json(item))).collect();
    uniq_keys(&keys, trace)
}

#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "the validator charges each comparison through its own uniq; the tests check this one against the recorded sorts"
    )
)]
fn uniq_keys(keys: &[PyValue], trace: Option<&mut Vec<(usize, usize)>>) -> bool {
    match sorted_indices(keys, trace) {
        Ok(order) => !order.windows(2).any(|pair| equal(&keys[pair[0]], &keys[pair[1]])),
        Err(TypeError) => {
            for (n, e) in keys.iter().enumerate() {
                if keys[..n].iter().any(|seen| equal(seen, e)) {
                    return false;
                }
            }
            true
        }
    }
}

/// `sorted(keys)` as the permutation CPython 3.12 `list.sort` produces, or the `TypeError` it
/// raises.
pub(crate) fn sorted_indices(
    keys: &[PyValue],
    trace: Option<&mut Vec<(usize, usize)>>,
) -> Result<Vec<usize>, TypeError> {
    let mut sorter = Sorter::new(Cmp {
        keys,
        trace,
        room: None,
        recursed: false,
    });
    sorter.sort()?;
    Ok(sorter.arr)
}

/// [`sorted_indices`] with `room` C recursion checks left when `list.sort` compares two keys.
pub(crate) fn sorted_within(keys: &[PyValue], room: usize) -> Result<Vec<usize>, SortError> {
    let mut sorter = Sorter::new(Cmp {
        keys,
        trace: None,
        room: Some((room, checked(keys))),
        recursed: false,
    });
    match sorter.sort() {
        Ok(()) => Ok(sorter.arr),
        Err(TypeError) if sorter.cmp.recursed => Err(SortError::Recursion),
        Err(TypeError) => Err(SortError::Type),
    }
}

struct Cmp<'a> {
    keys: &'a [PyValue],
    trace: Option<&'a mut Vec<(usize, usize)>>,
    /// The C recursion checks left and whether `list.sort` compares through `PyObject_RichCompareBool`, when the
    /// comparisons count them.
    room: Option<(usize, bool)>,
    /// Whether a comparison raised `RecursionError`, which stops the sort as a `TypeError` does.
    recursed: bool,
}

impl Cmp<'_> {
    fn lt(&mut self, x: usize, y: usize) -> Result<bool, TypeError> {
        if let Some(trace) = self.trace.as_mut() {
            trace.push((x, y));
        }
        let (v, w) = (&self.keys[x], &self.keys[y]);
        let Some((room, checked)) = self.room else {
            return py_lt(v, w);
        };
        let compare = Compare {
            room,
            pending: Vec::new(),
            unequal: HashMap::new(),
        };
        compare.lt(v, w, checked).map_err(|error| {
            self.recursed = error == SortError::Recursion;
            TypeError
        })
    }
}

#[derive(Clone, Copy)]
struct Run {
    base: usize,
    len: usize,
    power: usize,
}

struct Sorter<'a> {
    arr: Vec<usize>,
    cmp: Cmp<'a>,
    min_gallop: usize,
    pending: Vec<Run>,
    listlen: usize,
}

/// CPython `gallop_left`; `a` is the run starting at its first element.
fn gallop_left(cmp: &mut Cmp<'_>, key: usize, a: &[usize], n: usize, hint: usize) -> Result<usize, TypeError> {
    let (n, hint) = (n as isize, hint as isize);
    let mut ofs: isize = 1;
    let mut lastofs: isize = 0;
    if cmp.lt(a[hint as usize], key)? {
        let maxofs = n - hint;
        while ofs < maxofs {
            if cmp.lt(a[(hint + ofs) as usize], key)? {
                lastofs = ofs;
                ofs = (ofs << 1) + 1;
            } else {
                break;
            }
        }
        if ofs > maxofs {
            ofs = maxofs;
        }
        lastofs += hint;
        ofs += hint;
    } else {
        let maxofs = hint + 1;
        while ofs < maxofs {
            if cmp.lt(a[(hint - ofs) as usize], key)? {
                break;
            }
            lastofs = ofs;
            ofs = (ofs << 1) + 1;
        }
        if ofs > maxofs {
            ofs = maxofs;
        }
        let k = lastofs;
        lastofs = hint - ofs;
        ofs = hint - k;
    }
    lastofs += 1;
    while lastofs < ofs {
        let m = lastofs + ((ofs - lastofs) >> 1);
        if cmp.lt(a[m as usize], key)? {
            lastofs = m + 1;
        } else {
            ofs = m;
        }
    }
    Ok(ofs as usize)
}

/// CPython `gallop_right`; `a` is the run starting at its first element.
fn gallop_right(cmp: &mut Cmp<'_>, key: usize, a: &[usize], n: usize, hint: usize) -> Result<usize, TypeError> {
    let (n, hint) = (n as isize, hint as isize);
    let mut ofs: isize = 1;
    let mut lastofs: isize = 0;
    if cmp.lt(key, a[hint as usize])? {
        let maxofs = hint + 1;
        while ofs < maxofs {
            if cmp.lt(key, a[(hint - ofs) as usize])? {
                lastofs = ofs;
                ofs = (ofs << 1) + 1;
            } else {
                break;
            }
        }
        if ofs > maxofs {
            ofs = maxofs;
        }
        let k = lastofs;
        lastofs = hint - ofs;
        ofs = hint - k;
    } else {
        let maxofs = n - hint;
        while ofs < maxofs {
            if cmp.lt(key, a[(hint + ofs) as usize])? {
                break;
            }
            lastofs = ofs;
            ofs = (ofs << 1) + 1;
        }
        if ofs > maxofs {
            ofs = maxofs;
        }
        lastofs += hint;
        ofs += hint;
    }
    lastofs += 1;
    while lastofs < ofs {
        let m = lastofs + ((ofs - lastofs) >> 1);
        if cmp.lt(key, a[m as usize])? {
            ofs = m;
        } else {
            lastofs = m + 1;
        }
    }
    Ok(ofs as usize)
}

/// CPython `powerloop`: the powersort "power" of the boundary between two adjacent runs.
fn powerloop(s1: usize, n1: usize, n2: usize, n: usize) -> usize {
    let mut result = 0;
    let mut a = 2 * s1 + n1;
    let mut b = a + n1 + n2;
    loop {
        result += 1;
        if a >= n {
            a -= n;
            b -= n;
        } else if b >= n {
            break;
        }
        a <<= 1;
        b <<= 1;
    }
    result
}

/// CPython `merge_compute_minrun`.
fn merge_compute_minrun(mut n: usize) -> usize {
    let mut r = 0;
    while n >= 64 {
        r |= n & 1;
        n >>= 1;
    }
    n + r
}

impl<'a> Sorter<'a> {
    fn new(cmp: Cmp<'a>) -> Self {
        let n = cmp.keys.len();
        Self {
            arr: (0..n).collect(),
            cmp,
            min_gallop: MIN_GALLOP,
            pending: Vec::new(),
            listlen: n,
        }
    }

    fn sort(&mut self) -> Result<(), TypeError> {
        let mut nremaining = self.listlen;
        if nremaining < 2 {
            return Ok(());
        }
        let minrun = merge_compute_minrun(nremaining);
        let mut lo = 0;
        loop {
            let (mut n, descending) = self.count_run(lo, lo + nremaining)?;
            if descending {
                self.arr[lo..lo + n].reverse();
            }
            if n < minrun {
                let force = nremaining.min(minrun);
                self.binarysort(lo, lo + force, lo + n)?;
                n = force;
            }
            self.found_new_run(n)?;
            self.pending.push(Run {
                base: lo,
                len: n,
                power: 0,
            });
            lo += n;
            nremaining -= n;
            if nremaining == 0 {
                break;
            }
        }
        self.merge_force_collapse()
    }

    fn count_run(&mut self, lo: usize, hi: usize) -> Result<(usize, bool), TypeError> {
        let mut lo = lo + 1;
        if lo == hi {
            return Ok((1, false));
        }
        let mut n = 2;
        let descending = self.cmp.lt(self.arr[lo], self.arr[lo - 1])?;
        lo += 1;
        if descending {
            while lo < hi {
                if !self.cmp.lt(self.arr[lo], self.arr[lo - 1])? {
                    break;
                }
                lo += 1;
                n += 1;
            }
        } else {
            while lo < hi {
                if self.cmp.lt(self.arr[lo], self.arr[lo - 1])? {
                    break;
                }
                lo += 1;
                n += 1;
            }
        }
        Ok((n, descending))
    }

    fn binarysort(&mut self, lo: usize, hi: usize, start: usize) -> Result<(), TypeError> {
        // CPython first bumps `start` when `lo == start`; `count_run` always returns at least 1,
        // so `start > lo` here and that branch is unreachable.
        let mut start = start;
        while start < hi {
            let mut l = lo;
            let mut r = start;
            let pivot = self.arr[r];
            loop {
                let p = l + ((r - l) >> 1);
                if self.cmp.lt(pivot, self.arr[p])? {
                    r = p;
                } else {
                    l = p + 1;
                }
                if l >= r {
                    break;
                }
            }
            self.arr.copy_within(l..start, l + 1);
            self.arr[l] = pivot;
            start += 1;
        }
        Ok(())
    }

    fn found_new_run(&mut self, n2: usize) -> Result<(), TypeError> {
        let Some(last) = self.pending.last().copied() else {
            return Ok(());
        };
        let power = powerloop(last.base, last.len, n2, self.listlen);
        while self.pending.len() > 1 && self.pending[self.pending.len() - 2].power > power {
            self.merge_at(self.pending.len() - 2)?;
        }
        if let Some(top) = self.pending.last_mut() {
            top.power = power;
        }
        Ok(())
    }

    fn merge_force_collapse(&mut self) -> Result<(), TypeError> {
        while self.pending.len() > 1 {
            let mut n = self.pending.len() - 2;
            if n > 0 && self.pending[n - 1].len < self.pending[n + 1].len {
                n -= 1;
            }
            self.merge_at(n)?;
        }
        Ok(())
    }

    fn merge_at(&mut self, i: usize) -> Result<(), TypeError> {
        let (mut ssa, mut na) = (self.pending[i].base, self.pending[i].len);
        let (ssb, mut nb) = (self.pending[i + 1].base, self.pending[i + 1].len);
        self.pending[i].len = na + nb;
        if i + 3 == self.pending.len() {
            self.pending[i + 1] = self.pending[i + 2];
        }
        self.pending.pop();
        let k = gallop_right(&mut self.cmp, self.arr[ssb], &self.arr[ssa..], na, 0)?;
        ssa += k;
        na -= k;
        if na == 0 {
            return Ok(());
        }
        nb = gallop_left(&mut self.cmp, self.arr[ssa + na - 1], &self.arr[ssb..], nb, nb - 1)?;
        if nb == 0 {
            return Ok(());
        }
        if na <= nb {
            self.merge_lo(ssa, na, ssb, nb)
        } else {
            self.merge_hi(ssa, na, ssb, nb)
        }
    }

    fn merge_lo(&mut self, ssa: usize, mut na: usize, ssb: usize, mut nb: usize) -> Result<(), TypeError> {
        let tmp: Vec<usize> = self.arr[ssa..ssa + na].to_vec();
        let (mut ia, mut ib, mut dest) = (0, ssb, ssa);
        self.arr[dest] = self.arr[ib];
        dest += 1;
        ib += 1;
        nb -= 1;
        if nb == 0 {
            self.lo_succeed(&tmp, ia, na, dest);
            return Ok(());
        }
        if na == 1 {
            self.lo_copy_b(&tmp, ia, ib, nb, dest);
            return Ok(());
        }
        let mut min_gallop = self.min_gallop;
        loop {
            let mut acount = 0;
            let mut bcount = 0;
            loop {
                if self.cmp.lt(self.arr[ib], tmp[ia])? {
                    self.arr[dest] = self.arr[ib];
                    dest += 1;
                    ib += 1;
                    bcount += 1;
                    acount = 0;
                    nb -= 1;
                    if nb == 0 {
                        self.lo_succeed(&tmp, ia, na, dest);
                        return Ok(());
                    }
                    if bcount >= min_gallop {
                        break;
                    }
                } else {
                    self.arr[dest] = tmp[ia];
                    dest += 1;
                    ia += 1;
                    acount += 1;
                    bcount = 0;
                    na -= 1;
                    if na == 1 {
                        self.lo_copy_b(&tmp, ia, ib, nb, dest);
                        return Ok(());
                    }
                    if acount >= min_gallop {
                        break;
                    }
                }
            }
            min_gallop += 1;
            loop {
                if min_gallop > 1 {
                    min_gallop -= 1;
                }
                self.min_gallop = min_gallop;
                let k = gallop_right(&mut self.cmp, self.arr[ib], &tmp[ia..], na, 0)?;
                acount = k;
                if k != 0 {
                    self.arr[dest..dest + k].copy_from_slice(&tmp[ia..ia + k]);
                    dest += k;
                    ia += k;
                    na -= k;
                    if na == 1 {
                        self.lo_copy_b(&tmp, ia, ib, nb, dest);
                        return Ok(());
                    }
                    if na == 0 {
                        self.lo_succeed(&tmp, ia, na, dest);
                        return Ok(());
                    }
                }
                self.arr[dest] = self.arr[ib];
                dest += 1;
                ib += 1;
                nb -= 1;
                if nb == 0 {
                    self.lo_succeed(&tmp, ia, na, dest);
                    return Ok(());
                }
                let k = gallop_left(&mut self.cmp, tmp[ia], &self.arr[ib..], nb, 0)?;
                bcount = k;
                if k != 0 {
                    self.arr.copy_within(ib..ib + k, dest);
                    dest += k;
                    ib += k;
                    nb -= k;
                    if nb == 0 {
                        self.lo_succeed(&tmp, ia, na, dest);
                        return Ok(());
                    }
                }
                self.arr[dest] = tmp[ia];
                dest += 1;
                ia += 1;
                na -= 1;
                if na == 1 {
                    self.lo_copy_b(&tmp, ia, ib, nb, dest);
                    return Ok(());
                }
                if acount < MIN_GALLOP && bcount < MIN_GALLOP {
                    break;
                }
            }
            min_gallop += 1;
            self.min_gallop = min_gallop;
        }
    }

    fn lo_succeed(&mut self, tmp: &[usize], ia: usize, na: usize, dest: usize) {
        self.arr[dest..dest + na].copy_from_slice(&tmp[ia..ia + na]);
    }

    fn lo_copy_b(&mut self, tmp: &[usize], ia: usize, ib: usize, nb: usize, dest: usize) {
        self.arr.copy_within(ib..ib + nb, dest);
        self.arr[dest + nb] = tmp[ia];
    }

    fn merge_hi(&mut self, ssa: usize, mut na: usize, ssb: usize, mut nb: usize) -> Result<(), TypeError> {
        let tmp: Vec<usize> = self.arr[ssb..ssb + nb].to_vec();
        let basea = ssa;
        let mut dest = (ssa + na + nb) as isize - 1;
        let mut ia = (ssa + na) as isize - 1;
        let mut ib = nb as isize - 1;
        self.arr[dest as usize] = self.arr[ia as usize];
        dest -= 1;
        ia -= 1;
        na -= 1;
        // CPython tests `na == 0` here; `merge_hi` only runs with `na > nb >= 1`, so it cannot hold.
        if nb == 1 {
            self.hi_copy_a(&tmp, ib, ia, na, dest);
            return Ok(());
        }
        let mut min_gallop = self.min_gallop;
        loop {
            let mut acount = 0;
            let mut bcount = 0;
            loop {
                if self.cmp.lt(tmp[ib as usize], self.arr[ia as usize])? {
                    self.arr[dest as usize] = self.arr[ia as usize];
                    dest -= 1;
                    ia -= 1;
                    acount += 1;
                    bcount = 0;
                    na -= 1;
                    if na == 0 {
                        self.hi_succeed(&tmp, nb, dest);
                        return Ok(());
                    }
                    if acount >= min_gallop {
                        break;
                    }
                } else {
                    self.arr[dest as usize] = tmp[ib as usize];
                    dest -= 1;
                    ib -= 1;
                    bcount += 1;
                    acount = 0;
                    nb -= 1;
                    if nb == 1 {
                        self.hi_copy_a(&tmp, ib, ia, na, dest);
                        return Ok(());
                    }
                    if bcount >= min_gallop {
                        break;
                    }
                }
            }
            min_gallop += 1;
            loop {
                if min_gallop > 1 {
                    min_gallop -= 1;
                }
                self.min_gallop = min_gallop;
                let k = gallop_right(&mut self.cmp, tmp[ib as usize], &self.arr[basea..], na, na - 1)?;
                let k = na - k;
                acount = k;
                if k != 0 {
                    dest -= k as isize;
                    ia -= k as isize;
                    let (from, to) = ((ia + 1) as usize, (dest + 1) as usize);
                    self.arr.copy_within(from..from + k, to);
                    na -= k;
                    if na == 0 {
                        self.hi_succeed(&tmp, nb, dest);
                        return Ok(());
                    }
                }
                self.arr[dest as usize] = tmp[ib as usize];
                dest -= 1;
                ib -= 1;
                nb -= 1;
                if nb == 1 {
                    self.hi_copy_a(&tmp, ib, ia, na, dest);
                    return Ok(());
                }
                let k = gallop_left(&mut self.cmp, self.arr[ia as usize], &tmp, nb, nb - 1)?;
                let k = nb - k;
                bcount = k;
                if k != 0 {
                    dest -= k as isize;
                    ib -= k as isize;
                    let (from, to) = ((ib + 1) as usize, (dest + 1) as usize);
                    self.arr[to..to + k].copy_from_slice(&tmp[from..from + k]);
                    nb -= k;
                    if nb == 1 {
                        self.hi_copy_a(&tmp, ib, ia, na, dest);
                        return Ok(());
                    }
                    if nb == 0 {
                        self.hi_succeed(&tmp, nb, dest);
                        return Ok(());
                    }
                }
                self.arr[dest as usize] = self.arr[ia as usize];
                dest -= 1;
                ia -= 1;
                na -= 1;
                if na == 0 {
                    self.hi_succeed(&tmp, nb, dest);
                    return Ok(());
                }
                if acount < MIN_GALLOP && bcount < MIN_GALLOP {
                    break;
                }
            }
            min_gallop += 1;
            self.min_gallop = min_gallop;
        }
    }

    fn hi_succeed(&mut self, tmp: &[usize], nb: usize, dest: isize) {
        let start = (dest + 1) as usize - nb;
        self.arr[start..start + nb].copy_from_slice(&tmp[..nb]);
    }

    fn hi_copy_a(&mut self, tmp: &[usize], ib: isize, ia: isize, na: usize, dest: isize) {
        let dest = dest - na as isize;
        let ia = ia - na as isize;
        let (from, to) = ((ia + 1) as usize, (dest + 1) as usize);
        self.arr.copy_within(from..from + na, to);
        self.arr[dest as usize] = tmp[ib as usize];
    }
}

#[cfg(test)]
#[path = "python_sort_tests.rs"]
mod tests;
