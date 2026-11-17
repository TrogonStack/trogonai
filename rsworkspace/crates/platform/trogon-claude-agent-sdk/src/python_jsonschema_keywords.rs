//! The keyword functions of `jsonschema._keywords` and `jsonschema._legacy_keywords`, with the
//! `find_evaluated_item_indexes_by_schema` and `find_evaluated_property_keys_by_schema` helpers that
//! `unevaluatedItems` and `unevaluatedProperties` use (`jsonschema._utils` for draft 2020-12 and
//! `jsonschema._legacy_keywords` for draft 2019-09).

use super::{
    Abc, BUILTIN_CALL, Engine, Flow, Place, PyError, RESUMED, Sink, Site, Validator,
    drafts::Keyword,
    ops::{self, Op},
    referencing::{RefError, Resolved},
};
use crate::constants::{C_RECURSION_LIMIT, COMPARE_RECURSION_ERROR};
use crate::{
    python_arith::fails_multiple_of,
    python_json_value::{PythonJsonObject, PythonJsonValue as Value},
    python_pprint::PathSegment,
    python_re::py_repr,
    python_set,
    python_sort::{PyValue, SortError, equal, identical, py_eq, sorted_within, unbool},
    python_string::PythonString,
    python_value_repr::repr,
};
use std::borrow::Cow;

/// A `find_evaluated_*` helper, which the steps they share recurse into.
type Find<'s, T> = for<'v> fn(&Engine<'s>, &Validator<'v>, &Value, &Value) -> Result<Vec<T>, PythonString>;

/// A string step of an error path.
fn key(name: &PythonString) -> PathSegment {
    PathSegment::Key(name.clone())
}

/// The Python strings `names`, as values to `repr`.
fn strings<'a>(names: impl IntoIterator<Item = &'a PythonString>) -> Vec<Value> {
    names.into_iter().map(|name| Value::String(name.clone())).collect()
}

/// `jsonschema._utils.ensure_list`, followed by the iteration every caller does.
fn ensure_list(value: &Value) -> Result<Vec<Cow<'_, Value>>, String> {
    if value.is_string() {
        Ok(vec![Cow::Borrowed(value)])
    } else {
        ops::iterate(value)
    }
}

/// `type(value).__name__`; an `unbool` sentinel is an `object()`.
fn py_type_name(value: &PyValue) -> &'static str {
    match value {
        PyValue::None => "NoneType",
        PyValue::Bool(_) => "bool",
        PyValue::Int(_) | PyValue::Big(_) => "int",
        PyValue::Float(_) => "float",
        PyValue::Str(_) => "str",
        PyValue::List(_) => "list",
        PyValue::Dict(_) => "dict",
        PyValue::Sentinel(_) => "object",
    }
}

/// How a `minItems`-like keyword compares a length with its value and words its error.
struct Size {
    op: Op,
    /// The value that picks `edge_word`.
    edge: i128,
    edge_word: &'static str,
    word: &'static str,
}

impl Size {
    const MIN_SIZE: Self = Self {
        op: Op::Lt,
        edge: 1,
        edge_word: "should be non-empty",
        word: "is too short",
    };

    const MAX_SIZE: Self = Self {
        op: Op::Gt,
        edge: 0,
        edge_word: "is expected to be empty",
        word: "is too long",
    };

    const MIN_PROPERTIES: Self = Self {
        word: "does not have enough properties",
        ..Self::MIN_SIZE
    };

    const MAX_PROPERTIES: Self = Self {
        word: "has too many properties",
        ..Self::MAX_SIZE
    };
}

/// `_keywords.dependentRequired` once the instance passed the `object` check.
fn dependent_required(dependencies: &Value, object: &PythonJsonObject, instance: &Value, sink: &mut Sink) -> Flow {
    for (property, dependency) in ops::items(dependencies)? {
        if object.contains_key(property) {
            required_by(property, dependency, instance, sink)?;
        }
    }
    Ok(())
}

/// The `for each in dependency: if each not in instance` loop of the dependency keywords.
fn required_by(property: &PythonString, dependency: &Value, instance: &Value, sink: &mut Sink) -> Flow {
    for each in ops::iterate(dependency)? {
        if !ops::contains(instance, &each)? {
            sink.emit(PyError::new(format!(
                "{} is a dependency of {}",
                repr(&each),
                py_repr(property)
            )))?;
        }
    }
    Ok(())
}

/// `_keywords.required` once the instance passed the `object` check.
fn required(required: &Value, instance: &Value, sink: &mut Sink) -> Flow {
    for property in ops::iterate(required)? {
        if !ops::contains(instance, &property)? {
            sink.emit(PyError::new(format!("{} is a required property", repr(&property))))?;
        }
    }
    Ok(())
}

impl<'s> Engine<'s> {
    /// `_keywords.format` and `FormatChecker.check` with the class `FORMAT_CHECKER` when the validator has
    /// one. Only `check_schema` passes one, so the formats are those of the metaschemas: `regex`, checked by
    /// `is_regex`, and `uri` and `uri-reference`, which the pinned environment registers no check for.
    fn format(&self, format: &Value, instance: &Value, sink: &mut Sink) -> Flow {
        if !self.format_checker {
            return Ok(());
        }
        // `FormatChecker.check`, which looks the format up.
        self.peek(1)?;
        if let Some(error) = ops::unhashable(format) {
            return Err(error.into());
        }
        if format.as_str() != Some("regex") {
            return Ok(());
        }
        // `is_regex`, which compiles from the frame above.
        self.peek(2)?;
        if let Value::String(pattern) = instance
            && !self.compiles(pattern, 3)?
        {
            return sink.emit(PyError::new(format!("{} is not a {}", repr(instance), repr(format))));
        }
        Ok(())
    }

    /// The instance as an object once `validator.is_type(instance, "object")` passes.
    fn object_of<'i>(&self, v: &Validator<'_>, instance: &'i Value) -> Result<Option<&'i PythonJsonObject>, String> {
        let is_object = self.is_kind(v, instance, "object")?;
        Ok(instance.as_object().filter(|_| is_object))
    }

    /// The instance as an array once `validator.is_type(instance, "array")` passes.
    fn array_of<'i>(&self, v: &Validator<'_>, instance: &'i Value) -> Result<Option<&'i Vec<Value>>, String> {
        let is_array = self.is_kind(v, instance, "array")?;
        Ok(instance.as_array().filter(|_| is_array))
    }

    /// `minItems`, `maxItems`, `minLength`, `maxLength`, `minProperties` and `maxProperties`: the keyword checks
    /// that the instance is a `kind`, whose `len(instance)` is `count`.
    #[allow(
        clippy::too_many_arguments,
        reason = "the keyword function's arguments plus how it measures its type"
    )]
    fn size(
        &self,
        v: &Validator<'_>,
        kind: &str,
        count: Option<usize>,
        rule: &Size,
        limit: &Value,
        instance: &Value,
        sink: &mut Sink,
    ) -> Flow {
        let is_kind = self.is_kind(v, instance, kind)?;
        let Some(count) = count.filter(|_| is_kind) else {
            return Ok(());
        };
        if !ops::compare_count(count, rule.op, limit)? {
            return Ok(());
        }
        let word = if ops::equals_int(limit, rule.edge) {
            rule.edge_word
        } else {
            rule.word
        };
        sink.emit(PyError::new(format!("{} {word}", self.shown(instance)?)))
    }

    /// `minimum`, `maximum`, `exclusiveMinimum` and `exclusiveMaximum`: `instance OP limit` fails a number.
    fn bound(&self, v: &Validator<'_>, instance: &Value, op: Op, limit: &Value, words: &str, sink: &mut Sink) -> Flow {
        if self.is_kind(v, instance, "number")? && ops::compare(instance, op, limit)? {
            return sink.emit(PyError::new(format!(
                "{} is {words} of {}",
                repr(instance),
                repr(limit)
            )));
        }
        Ok(())
    }

    /// `_keywords.multipleOf`; the divisor is a number by then, so its `str()` is its `repr()`.
    fn multiple_of(&self, v: &Validator<'_>, divisor: &Value, instance: &Value, sink: &mut Sink) -> Flow {
        if self.is_kind(v, instance, "number")? && fails_multiple_of(instance, divisor)? {
            return sink.emit(PyError::new(format!(
                "{} is not a multiple of {}",
                repr(instance),
                repr(divisor)
            )));
        }
        Ok(())
    }

    /// `_utils.equal(one, two)`, called from the frame below: identical objects are equal at once, strings compare
    /// with `==`, sequences and mappings recurse through `_sequence_equal` and `_mapping_equal` and the generators
    /// their `all` call drains, and anything else compares after `unbool`.
    fn equal(&self, one: &PyValue, two: &PyValue) -> Result<bool, String> {
        if identical(one, two) {
            return Ok(true);
        }
        let (one_type, two_type) = (py_type_name(one), py_type_name(two));
        if one_type == "str" || two_type == "str" {
            return Ok(py_eq(one, two));
        }
        self.abc(one_type, Abc::Sequence)?;
        if let PyValue::List(left) = one {
            self.abc(two_type, Abc::Sequence)?;
            if let PyValue::List(right) = two {
                let _sequence_equal = self.enter(1)?;
                if left.len() != right.len() {
                    return Ok(false);
                }
                let _generator = self.enter(1)?;
                let _all = self.hold(RESUMED + BUILTIN_CALL);
                for (left, right) in left.iter().zip(right) {
                    let _equal = self.enter(1)?;
                    if !self.equal(left, right)? {
                        return Ok(false);
                    }
                }
                return Ok(true);
            }
        }
        self.abc(one_type, Abc::Mapping)?;
        if let PyValue::Dict(left) = one {
            self.abc(two_type, Abc::Mapping)?;
            if let PyValue::Dict(right) = two {
                let _mapping_equal = self.enter(1)?;
                if left.len() != right.len() {
                    return Ok(false);
                }
                let _generator = self.enter(1)?;
                let _all = self.hold(RESUMED + BUILTIN_CALL);
                for (key, left) in left {
                    let Some((_, right)) = right.iter().find(|(other, _)| other == key) else {
                        return Ok(false);
                    };
                    let _equal = self.enter(1)?;
                    if !self.equal(left, right)? {
                        return Ok(false);
                    }
                }
                return Ok(true);
            }
        }
        // `unbool`.
        self.peek(1)?;
        Ok(equal(one, two))
    }

    /// `_utils.uniq(items)`, called from the frame below: the `unbool` items sorted and compared pairwise, or every
    /// pair when sorting raises `TypeError`. `sorted` sorts under the checks under way, its own call the first time
    /// and `list.sort`'s, so comparing keys nested past the checks left raises `RecursionError`.
    fn uniq(&self, items: &[Value]) -> Result<bool, String> {
        let keys: Vec<PyValue> = items.iter().map(|item| unbool(PyValue::from_json(item))).collect();
        let cold = self.first(Site::Uniq);
        let sorted = {
            // The generator `sorted` drains, which calls `unbool` for each item.
            let _generator = self.enter(1)?;
            if !keys.is_empty() {
                self.peek(1)?;
            }
            let held = self.c_held() + BUILTIN_CALL + usize::from(cold);
            sorted_within(&keys, C_RECURSION_LIMIT.saturating_sub(held))
        };
        match sorted {
            Ok(order) => {
                for pair in order.windows(2) {
                    let _equal = self.enter(1)?;
                    if self.equal(&keys[pair[0]], &keys[pair[1]])? {
                        return Ok(false);
                    }
                }
            }
            Err(SortError::Recursion) => return Err(COMPARE_RECURSION_ERROR.to_owned()),
            Err(SortError::Type) => {
                for (index, item) in keys.iter().enumerate() {
                    // `unbool`.
                    self.peek(1)?;
                    for seen in &keys[..index] {
                        let _equal = self.enter(1)?;
                        if self.equal(seen, item)? {
                            return Ok(false);
                        }
                    }
                }
            }
        }
        Ok(true)
    }

    /// `_keywords.const`.
    fn constant(&self, expected: &Value, instance: &Value, sink: &mut Sink) -> Flow {
        let matched = {
            let _equal = self.enter(1)?;
            self.equal(&PyValue::from_json(instance), &PyValue::from_json(expected))?
        };
        if matched {
            return Ok(());
        }
        sink.emit(PyError::new(format!("{} was expected", self.shown(expected)?)))
    }

    /// `_keywords.enum`: `all(not equal(each, instance) for each in enums)`.
    fn enumeration(&self, enums: &Value, instance: &Value, sink: &mut Sink) -> Flow {
        let wanted = PyValue::from_json(instance);
        let each = ops::iterate(enums)?;
        let found = {
            let _generator = self.enter(1)?;
            let _all = self.hold(RESUMED + BUILTIN_CALL);
            let mut found = false;
            for each in &each {
                let _equal = self.enter(1)?;
                if self.equal(&PyValue::from_json(each), &wanted)? {
                    found = true;
                    break;
                }
            }
            found
        };
        if found {
            return Ok(());
        }
        sink.emit(PyError::new(format!(
            "{} is not one of {}",
            self.shown(instance)?,
            self.shown(enums)?
        )))
    }

    /// `_keywords.uniqueItems`.
    fn unique_items(&self, v: &Validator<'_>, unique: &Value, instance: &Value, sink: &mut Sink) -> Flow {
        if !ops::truthy(unique) {
            return Ok(());
        }
        let Some(items) = self.array_of(v, instance)? else {
            return Ok(());
        };
        let unique = {
            let _uniq = self.enter(1)?;
            self.uniq(items)?
        };
        if unique {
            return Ok(());
        }
        sink.emit(PyError::new(format!(
            "{} has non-unique elements",
            self.shown(instance)?
        )))
    }

    /// `_keywords.pattern`.
    fn pattern(&self, v: &Validator<'_>, pattern: &Value, instance: &Value, sink: &mut Sink) -> Flow {
        if self.is_kind(v, instance, "string")?
            && let Value::String(text) = instance
            && !self.search(pattern, text)?
        {
            return sink.emit(PyError::new(format!(
                "{} does not match {}",
                repr(instance),
                repr(pattern)
            )));
        }
        Ok(())
    }

    /// Runs the keyword function `keyword` of `v`, whose schema holds `value` under the keyword.
    pub(super) fn keyword(
        &self,
        keyword: Keyword,
        v: &Validator<'_>,
        value: &Value,
        instance: &Value,
        sink: &mut Sink,
    ) -> Flow {
        match keyword {
            Keyword::AdditionalItems => self.additional_items(v, value, instance, sink),
            Keyword::AdditionalProperties => self.additional_properties(v, value, instance, sink),
            Keyword::AllOf => self.all_of(v, value, instance, sink),
            Keyword::AnyOf => self.any_of(v, value, instance, sink),
            Keyword::Const => self.constant(value, instance, sink),
            Keyword::Contains => self.contains(v, value, instance, sink),
            Keyword::ContainsDraft6Draft7 => self.contains_draft6_draft7(v, value, instance, sink),
            Keyword::DependenciesDraft3 => self.dependencies_draft3(v, value, instance, sink),
            Keyword::DependenciesDraft4Draft6Draft7 => self.dependencies(v, value, instance, sink),
            Keyword::DependentRequired => match self.object_of(v, instance)? {
                Some(object) => dependent_required(value, object, instance, sink),
                None => Ok(()),
            },
            Keyword::DependentSchemas => self.dependent_schemas(v, value, instance, sink),
            Keyword::DisallowDraft3 => self.disallow_draft3(v, value, instance, sink),
            Keyword::DynamicRef | Keyword::Ref => self.reference(v, value, instance, sink),
            Keyword::Enum => self.enumeration(value, instance, sink),
            Keyword::ExclusiveMaximum => {
                self.bound(v, instance, Op::Ge, value, "greater than or equal to the maximum", sink)
            }
            Keyword::ExclusiveMinimum => {
                self.bound(v, instance, Op::Le, value, "less than or equal to the minimum", sink)
            }
            Keyword::ExtendsDraft3 => self.extends_draft3(v, value, instance, sink),
            Keyword::Format => self.format(value, instance, sink),
            Keyword::If => self.if_then_else(v, value, instance, sink),
            Keyword::Items => self.items(v, value, instance, sink),
            Keyword::ItemsDraft3Draft4 => self.items_draft3_draft4(v, value, instance, sink),
            Keyword::ItemsDraft6Draft7Draft201909 => self.items_draft6_draft7_draft201909(v, value, instance, sink),
            Keyword::MaxItems => self.size(
                v,
                "array",
                instance.as_array().map(Vec::len),
                &Size::MAX_SIZE,
                value,
                instance,
                sink,
            ),
            Keyword::MaxLength => self.size(
                v,
                "string",
                instance.as_python_str().map(PythonString::len),
                &Size::MAX_SIZE,
                value,
                instance,
                sink,
            ),
            Keyword::MaxProperties => self.size(
                v,
                "object",
                instance.as_object().map(PythonJsonObject::len),
                &Size::MAX_PROPERTIES,
                value,
                instance,
                sink,
            ),
            Keyword::Maximum => self.bound(v, instance, Op::Gt, value, "greater than the maximum", sink),
            Keyword::MaximumDraft3Draft4 => {
                if v.schema.get("exclusiveMaximum").is_some_and(ops::truthy) {
                    self.bound(v, instance, Op::Ge, value, "greater than or equal to the maximum", sink)
                } else {
                    self.bound(v, instance, Op::Gt, value, "greater than the maximum", sink)
                }
            }
            Keyword::MinItems => self.size(
                v,
                "array",
                instance.as_array().map(Vec::len),
                &Size::MIN_SIZE,
                value,
                instance,
                sink,
            ),
            Keyword::MinLength => self.size(
                v,
                "string",
                instance.as_python_str().map(PythonString::len),
                &Size::MIN_SIZE,
                value,
                instance,
                sink,
            ),
            Keyword::MinProperties => self.size(
                v,
                "object",
                instance.as_object().map(PythonJsonObject::len),
                &Size::MIN_PROPERTIES,
                value,
                instance,
                sink,
            ),
            Keyword::Minimum => self.bound(v, instance, Op::Lt, value, "less than the minimum", sink),
            Keyword::MinimumDraft3Draft4 => {
                if v.schema.get("exclusiveMinimum").is_some_and(ops::truthy) {
                    self.bound(v, instance, Op::Le, value, "less than or equal to the minimum", sink)
                } else {
                    self.bound(v, instance, Op::Lt, value, "less than the minimum", sink)
                }
            }
            Keyword::MultipleOf => self.multiple_of(v, value, instance, sink),
            Keyword::Not => self.not(v, value, instance, sink),
            Keyword::OneOf => self.one_of(v, value, instance, sink),
            Keyword::Pattern => self.pattern(v, value, instance, sink),
            Keyword::PatternProperties => self.pattern_properties(v, value, instance, sink),
            Keyword::PrefixItems => self.prefix_items(v, value, instance, sink),
            Keyword::Properties => self.properties(v, value, instance, sink),
            Keyword::PropertiesDraft3 => self.properties_draft3(v, value, instance, sink),
            Keyword::PropertyNames => self.property_names(v, value, instance, sink),
            Keyword::RecursiveRef => self.recursive_reference(v, instance, sink),
            Keyword::Required => {
                if self.is_kind(v, instance, "object")? {
                    required(value, instance, sink)
                } else {
                    Ok(())
                }
            }
            Keyword::Type => self.type_keyword(v, value, instance, sink),
            Keyword::TypeDraft3 => self.type_draft3(v, value, instance, sink),
            Keyword::UnevaluatedItems => self.unevaluated_items(v, false, instance, sink),
            Keyword::UnevaluatedItemsDraft2019 => self.unevaluated_items(v, true, instance, sink),
            Keyword::UnevaluatedProperties => self.unevaluated_properties(v, false, value, instance, sink),
            Keyword::UnevaluatedPropertiesDraft2019 => self.unevaluated_properties(v, true, value, instance, sink),
            Keyword::UniqueItems => self.unique_items(v, value, instance, sink),
        }
    }

    /// `_keywords.ref` and `_keywords.dynamicRef`: `Validator._validate_reference`.
    fn reference(&self, v: &Validator<'_>, reference: &Value, instance: &Value, sink: &mut Sink) -> Flow {
        let Resolved { contents, resolver } = {
            // `_validate_reference` looks up, and returns the generator of `descend` the keyword then drains.
            let _validate_reference = self.enter(1)?;
            self.peek(self.lookup_frames(v, reference))?;
            v.resolver
                .lookup(&self.registry, reference)
                .map_err(RefError::wrapped)?
        };
        self.descend(v, instance, &contents, Place::default(), Some(resolver), sink)
    }

    /// `_legacy_keywords.recursiveRef`.
    fn recursive_reference(&self, v: &Validator<'_>, instance: &Value, sink: &mut Sink) -> Flow {
        let Resolved { contents, resolver } = {
            // `_RefResolver`-free `lookup_recursive_ref`, which the keyword calls before descending.
            let _lookup_recursive_ref = self.enter(1)?;
            self.peek(self.recursive_lookup_frames())?;
            v.resolver.lookup_recursive(&self.registry).map_err(RefError::raw)?
        };
        self.descend(v, instance, &contents, Place::default(), Some(resolver), sink)
    }

    /// `_keywords.additionalProperties`. `extras` is a `set`, so an object `additionalProperties` descends into them
    /// in the order the hash secret gives them.
    fn additional_properties(&self, v: &Validator<'_>, extra: &Value, instance: &Value, sink: &mut Sink) -> Flow {
        let Some(object) = self.object_of(v, instance)? else {
            return Ok(());
        };
        let extras = self.additional(object, v.schema)?;
        if self.is_kind(v, extra, "object")? {
            let names: Vec<&PythonString> = extras.iter().map(|&(name, _)| name).collect();
            for at in python_set::set_order(names.as_slice(), self.budget.secret) {
                let (name, child) = extras[at];
                self.descend(v, child, extra, Place::path(key(name)), None, sink)?;
            }
            return Ok(());
        }
        if ops::truthy(extra) || extras.is_empty() {
            return Ok(());
        }
        let mut names: Vec<&PythonString> = extras.into_iter().map(|(name, _)| name).collect();
        names.sort();
        let names = strings(names);
        let message = match v.schema.get("patternProperties") {
            Some(patterns) => {
                let verb = if names.len() == 1 { "does" } else { "do" };
                let patterns = ops::sorted_values(ops::iterate(patterns)?)?;
                format!(
                    "{} {verb} not match any of the regexes: {}",
                    self.join_reprs(&names)?,
                    self.join_reprs(patterns.iter().map(|each| &**each))?
                )
            }
            None => {
                let (joined, verb) = self.extras_msg(&names)?;
                format!("Additional properties are not allowed ({joined} {verb} unexpected)")
            }
        };
        sink.emit(PyError::new(message))
    }

    /// `jsonschema._utils.find_additional_properties`, drained by `set(...)`: the generator resumes one frame
    /// above the keyword, once before each property and once more to finish, and searches the joined patterns from
    /// there. The resumes are no deeper than the `validator.is_type(instance, "object")` the keyword ran from its
    /// own frame just before, so only the search can raise `RecursionError`.
    fn additional<'i>(
        &self,
        object: &'i PythonJsonObject,
        schema: &Value,
    ) -> Result<Vec<(&'i PythonString, &'i Value)>, PythonString> {
        let empty = Value::Object(PythonJsonObject::new());
        let properties = schema.get("properties").unwrap_or(&empty);
        let patterns = schema
            .get("patternProperties")
            .map_or(Ok(PythonString::new()), ops::join_patterns)?;
        let mut extras = Vec::new();
        for (name, child) in object {
            if !ops::has_name(properties, name)? {
                if !patterns.is_empty() && self.search_str(&patterns, name, 2)? {
                    continue;
                }
                extras.push((name, child));
            }
        }
        Ok(extras)
    }

    /// `_keywords.patternProperties`.
    fn pattern_properties(&self, v: &Validator<'_>, patterns: &Value, instance: &Value, sink: &mut Sink) -> Flow {
        let Some(object) = self.object_of(v, instance)? else {
            return Ok(());
        };
        for (pattern, subschema) in ops::items(patterns)? {
            for (name, child) in object {
                if self.search_str(pattern, name, 1)? {
                    self.descend(v, child, subschema, Place::both(key(name), key(pattern)), None, sink)?;
                }
            }
        }
        Ok(())
    }

    /// `_keywords.propertyNames`.
    fn property_names(&self, v: &Validator<'_>, names: &Value, instance: &Value, sink: &mut Sink) -> Flow {
        let Some(object) = self.object_of(v, instance)? else {
            return Ok(());
        };
        for name in object.keys() {
            self.descend(v, &Value::String(name.clone()), names, Place::default(), None, sink)?;
        }
        Ok(())
    }

    /// `_keywords.properties`.
    fn properties(&self, v: &Validator<'_>, properties: &Value, instance: &Value, sink: &mut Sink) -> Flow {
        let Some(object) = self.object_of(v, instance)? else {
            return Ok(());
        };
        for (property, subschema) in ops::items(properties)? {
            if let Some(child) = object.get(property) {
                self.descend(
                    v,
                    child,
                    subschema,
                    Place::both(key(property), key(property)),
                    None,
                    sink,
                )?;
            }
        }
        Ok(())
    }

    /// `_legacy_keywords.properties_draft3`.
    fn properties_draft3(&self, v: &Validator<'_>, properties: &Value, instance: &Value, sink: &mut Sink) -> Flow {
        let Some(object) = self.object_of(v, instance)? else {
            return Ok(());
        };
        for (property, subschema) in ops::items(properties)? {
            if let Some(child) = object.get(property) {
                self.descend(
                    v,
                    child,
                    subschema,
                    Place::both(key(property), key(property)),
                    None,
                    sink,
                )?;
            } else if let Some(flag) = ops::get(subschema, "required")?.filter(|flag| ops::truthy(flag)) {
                let mut error = PyError::keyword(
                    format!("{} is a required property", py_repr(property)),
                    "required",
                    flag,
                );
                error.instance = Some(instance.clone());
                error.schema = Some(v.schema.clone());
                error.path.push_front(key(property));
                error
                    .schema_path
                    .extend([key(property), PathSegment::Key("required".into())]);
                sink.emit(error)?;
            }
        }
        Ok(())
    }

    /// `_keywords.dependentSchemas`.
    fn dependent_schemas(&self, v: &Validator<'_>, dependencies: &Value, instance: &Value, sink: &mut Sink) -> Flow {
        let Some(object) = self.object_of(v, instance)? else {
            return Ok(());
        };
        for (property, dependency) in ops::items(dependencies)? {
            if object.contains_key(property) {
                self.descend(v, instance, dependency, Place::schema_path(key(property)), None, sink)?;
            }
        }
        Ok(())
    }

    /// `_legacy_keywords.dependencies_draft4_draft6_draft7`.
    fn dependencies(&self, v: &Validator<'_>, dependencies: &Value, instance: &Value, sink: &mut Sink) -> Flow {
        let Some(object) = self.object_of(v, instance)? else {
            return Ok(());
        };
        for (property, dependency) in ops::items(dependencies)? {
            if !object.contains_key(property) {
                continue;
            }
            if self.is_kind(v, dependency, "array")? {
                required_by(property, dependency, instance, sink)?;
            } else {
                self.descend(v, instance, dependency, Place::schema_path(key(property)), None, sink)?;
            }
        }
        Ok(())
    }

    /// `_legacy_keywords.dependencies_draft3`.
    fn dependencies_draft3(&self, v: &Validator<'_>, dependencies: &Value, instance: &Value, sink: &mut Sink) -> Flow {
        let Some(object) = self.object_of(v, instance)? else {
            return Ok(());
        };
        for (property, dependency) in ops::items(dependencies)? {
            if !object.contains_key(property) {
                continue;
            }
            if self.is_kind(v, dependency, "object")? {
                self.descend(v, instance, dependency, Place::schema_path(key(property)), None, sink)?;
            } else if self.is_kind(v, dependency, "string")? {
                if let Value::String(name) = dependency
                    && !object.contains_key(name)
                {
                    sink.emit(PyError::new(format!(
                        "{} is a dependency of {}",
                        repr(dependency),
                        py_repr(property)
                    )))?;
                }
            } else {
                required_by(property, dependency, instance, sink)?;
            }
        }
        Ok(())
    }

    /// `_keywords.items` of draft 2020-12.
    fn items(&self, v: &Validator<'_>, items: &Value, instance: &Value, sink: &mut Sink) -> Flow {
        let Some(array) = self.array_of(v, instance)? else {
            return Ok(());
        };
        let prefix = match v.schema.get("prefixItems") {
            Some(prefix_items) => ops::len(prefix_items)?,
            None => 0,
        };
        let total = array.len();
        if total <= prefix {
            return Ok(());
        }
        let extra = total - prefix;
        if *items == Value::Bool(false) {
            let rest = if extra == 1 {
                array.get(prefix).cloned().unwrap_or(Value::Null)
            } else {
                Value::Array(array.iter().skip(prefix).cloned().collect())
            };
            let noun = if prefix == 1 { "item" } else { "items" };
            return sink.emit(PyError::new(format!(
                "Expected at most {prefix} {noun} but found {extra} extra: {}",
                self.shown(&rest)?
            )));
        }
        for (index, item) in array.iter().enumerate().skip(prefix) {
            self.descend(v, item, items, Place::path(PathSegment::Index(index)), None, sink)?;
        }
        Ok(())
    }

    /// `_keywords.prefixItems`.
    fn prefix_items(&self, v: &Validator<'_>, prefix_items: &Value, instance: &Value, sink: &mut Sink) -> Flow {
        let Some(array) = self.array_of(v, instance)? else {
            return Ok(());
        };
        self.zip_items(v, array, &ops::iterate(prefix_items)?, sink)
    }

    /// `for (index, item), subschema in zip(enumerate(instance), subschemas)`, descending with `index` as both
    /// paths.
    fn zip_items(&self, v: &Validator<'_>, array: &[Value], subschemas: &[Cow<'_, Value>], sink: &mut Sink) -> Flow {
        for ((index, item), subschema) in array.iter().enumerate().zip(subschemas) {
            let place = Place::both(PathSegment::Index(index), PathSegment::Index(index));
            self.descend(v, item, subschema, place, None, sink)?;
        }
        Ok(())
    }

    /// Descends into every item with the same schema, with the item's index as its path.
    fn each_item(&self, v: &Validator<'_>, array: &[Value], schema: &Value, skip: usize, sink: &mut Sink) -> Flow {
        for (index, item) in array.iter().enumerate().skip(skip) {
            self.descend(v, item, schema, Place::path(PathSegment::Index(index)), None, sink)?;
        }
        Ok(())
    }

    /// `_legacy_keywords.items_draft3_draft4`.
    fn items_draft3_draft4(&self, v: &Validator<'_>, items: &Value, instance: &Value, sink: &mut Sink) -> Flow {
        let Some(array) = self.array_of(v, instance)? else {
            return Ok(());
        };
        if self.is_kind(v, items, "object")? {
            return self.each_item(v, array, items, 0, sink);
        }
        self.zip_items(v, array, &ops::iterate(items)?, sink)
    }

    /// `_legacy_keywords.items_draft6_draft7_draft201909`.
    fn items_draft6_draft7_draft201909(
        &self,
        v: &Validator<'_>,
        items: &Value,
        instance: &Value,
        sink: &mut Sink,
    ) -> Flow {
        let Some(array) = self.array_of(v, instance)? else {
            return Ok(());
        };
        let is_array = self.is_kind(v, items, "array")?;
        match items {
            Value::Array(subschemas) if is_array => {
                let subschemas: Vec<Cow<'_, Value>> = subschemas.iter().map(Cow::Borrowed).collect();
                self.zip_items(v, array, &subschemas, sink)
            }
            _ => self.each_item(v, array, items, 0, sink),
        }
    }

    /// `_legacy_keywords.additionalItems`.
    fn additional_items(&self, v: &Validator<'_>, extra: &Value, instance: &Value, sink: &mut Sink) -> Flow {
        let Some(array) = self.array_of(v, instance)? else {
            return Ok(());
        };
        let empty = Value::Object(PythonJsonObject::new());
        let items = v.schema.get("items").unwrap_or(&empty);
        if self.is_kind(v, items, "object")? {
            return Ok(());
        }
        let len_items = ops::len(items)?;
        if self.is_kind(v, extra, "object")? {
            return self.each_item(v, array, extra, len_items, sink);
        }
        if ops::truthy(extra) || array.len() <= len_items {
            return Ok(());
        }
        let (joined, verb) = self.extras_msg(array.iter().skip(len_items))?;
        sink.emit(PyError::new(format!(
            "Additional items are not allowed ({joined} {verb} unexpected)"
        )))
    }

    /// `_keywords.contains` of drafts 2019-09 and 2020-12.
    fn contains(&self, v: &Validator<'_>, contains: &Value, instance: &Value, sink: &mut Sink) -> Flow {
        let Some(array) = self.array_of(v, instance)? else {
            return Ok(());
        };
        let one = Value::from(1);
        let min_contains = v.schema.get("minContains").unwrap_or(&one);
        let length = Value::from(array.len());
        let max_contains = v.schema.get("maxContains").unwrap_or(&length);
        let checker = self.evolve(v, contains)?;
        let mut matches = 0_usize;
        for each in array {
            if !self.is_valid(&checker, each)? {
                continue;
            }
            matches += 1;
            if ops::compare_count(matches, Op::Gt, max_contains)? {
                let message = format!(
                    "Too many items match the given schema (expected at most {})",
                    repr(max_contains)
                );
                return sink.emit(PyError::keyword(message, "maxContains", max_contains));
            }
        }
        if !ops::compare_count(matches, Op::Lt, min_contains)? {
            return Ok(());
        }
        if matches == 0 {
            return sink.emit(PyError::new(format!(
                "{} does not contain items matching the given schema",
                self.shown(instance)?
            )));
        }
        let message = format!(
            "Too few items match the given schema (expected at least {} but only {matches} matched)",
            repr(min_contains)
        );
        sink.emit(PyError::keyword(message, "minContains", min_contains))
    }

    /// `_legacy_keywords.contains_draft6_draft7`.
    fn contains_draft6_draft7(&self, v: &Validator<'_>, contains: &Value, instance: &Value, sink: &mut Sink) -> Flow {
        let Some(array) = self.array_of(v, instance)? else {
            return Ok(());
        };
        let found = {
            // The generator `any` drains: the call and resuming the generator from C hold checks.
            let _generator = self.enter(1)?;
            let _any = self.hold(RESUMED + BUILTIN_CALL);
            let mut found = false;
            for element in array {
                let checker = self.evolve(v, contains)?;
                if self.is_valid(&checker, element)? {
                    found = true;
                    break;
                }
            }
            found
        };
        if found {
            return Ok(());
        }
        sink.emit(PyError::new(format!(
            "None of {} are valid under the given schema",
            self.shown(instance)?
        )))
    }

    /// `_keywords.allOf`.
    fn all_of(&self, v: &Validator<'_>, subschemas: &Value, instance: &Value, sink: &mut Sink) -> Flow {
        for (index, subschema) in ops::iterate(subschemas)?.iter().enumerate() {
            self.descend(
                v,
                instance,
                subschema,
                Place::schema_path(PathSegment::Index(index)),
                None,
                sink,
            )?;
        }
        Ok(())
    }

    /// `_keywords.anyOf`.
    fn any_of(&self, v: &Validator<'_>, subschemas: &Value, instance: &Value, sink: &mut Sink) -> Flow {
        let mut all_errors = Vec::new();
        for (index, subschema) in ops::iterate(subschemas)?.iter().enumerate() {
            let errors = self.descend_all(v, instance, subschema, Place::schema_path(PathSegment::Index(index)))?;
            if errors.is_empty() {
                return Ok(());
            }
            all_errors.extend(errors);
        }
        sink.emit(PyError::with_context(
            format!("{} is not valid under any of the given schemas", self.shown(instance)?),
            all_errors,
        ))
    }

    /// `_keywords.oneOf`.
    fn one_of(&self, v: &Validator<'_>, subschemas: &Value, instance: &Value, sink: &mut Sink) -> Flow {
        let subschemas = ops::iterate(subschemas)?;
        let mut rest = subschemas.iter().enumerate();
        let mut all_errors = Vec::new();
        let mut first_valid = None;
        for (index, subschema) in rest.by_ref() {
            let errors = self.descend_all(v, instance, subschema, Place::schema_path(PathSegment::Index(index)))?;
            if errors.is_empty() {
                first_valid = Some(subschema);
                break;
            }
            all_errors.extend(errors);
        }
        let Some(first_valid) = first_valid else {
            return sink.emit(PyError::with_context(
                format!("{} is not valid under any of the given schemas", self.shown(instance)?),
                all_errors,
            ));
        };
        let mut more_valid = Vec::new();
        for (_, subschema) in rest {
            let checker = self.evolve(v, subschema)?;
            if self.is_valid(&checker, instance)? {
                more_valid.push(&**subschema);
            }
        }
        if more_valid.is_empty() {
            return Ok(());
        }
        more_valid.push(&**first_valid);
        sink.emit(PyError::new(format!(
            "{} is valid under each of {}",
            self.shown(instance)?,
            self.join_reprs(more_valid)?
        )))
    }

    /// `_keywords.not_`.
    fn not(&self, v: &Validator<'_>, not_schema: &Value, instance: &Value, sink: &mut Sink) -> Flow {
        let checker = self.evolve(v, not_schema)?;
        if !self.is_valid(&checker, instance)? {
            return Ok(());
        }
        sink.emit(PyError::new(format!(
            "{} should not be valid under {}",
            self.shown(instance)?,
            self.shown(not_schema)?
        )))
    }

    /// `_keywords.if_`.
    fn if_then_else(&self, v: &Validator<'_>, if_schema: &Value, instance: &Value, sink: &mut Sink) -> Flow {
        let checker = self.evolve(v, if_schema)?;
        let branch = if self.is_valid(&checker, instance)? {
            "then"
        } else {
            "else"
        };
        match v.schema.get(branch) {
            Some(subschema) => {
                let place = Place::schema_path(PathSegment::Key(branch.into()));
                self.descend(v, instance, subschema, place, None, sink)
            }
            None => Ok(()),
        }
    }

    /// `_legacy_keywords.disallow_draft3`.
    fn disallow_draft3(&self, v: &Validator<'_>, disallow: &Value, instance: &Value, sink: &mut Sink) -> Flow {
        // `ensure_list`.
        self.peek(1)?;
        for disallowed in ensure_list(disallow)? {
            let probe = Value::Object(PythonJsonObject::from_iter([(
                PythonString::from("type"),
                Value::Array(vec![disallowed.as_ref().clone()]),
            )]));
            let checker = self.evolve(v, &probe)?;
            if self.is_valid(&checker, instance)? {
                sink.emit(PyError::new(format!(
                    "{} is disallowed for {}",
                    self.shown(&disallowed)?,
                    self.shown(instance)?
                )))?;
            }
        }
        Ok(())
    }

    /// `_legacy_keywords.extends_draft3`.
    fn extends_draft3(&self, v: &Validator<'_>, extends: &Value, instance: &Value, sink: &mut Sink) -> Flow {
        if self.is_kind(v, extends, "object")? {
            return self.descend(v, instance, extends, Place::default(), None, sink);
        }
        for (index, subschema) in ops::iterate(extends)?.iter().enumerate() {
            self.descend(
                v,
                instance,
                subschema,
                Place::schema_path(PathSegment::Index(index)),
                None,
                sink,
            )?;
        }
        Ok(())
    }

    /// `_legacy_keywords.type_draft3`.
    fn type_draft3(&self, v: &Validator<'_>, types: &Value, instance: &Value, sink: &mut Sink) -> Flow {
        // `ensure_list`.
        self.peek(1)?;
        let types = ensure_list(types)?;
        let mut all_errors = Vec::new();
        for (index, kind) in types.iter().enumerate() {
            if self.is_kind(v, kind, "object")? {
                let errors = self.descend_all(v, instance, kind, Place::schema_path(PathSegment::Index(index)))?;
                if errors.is_empty() {
                    return Ok(());
                }
                all_errors.extend(errors);
            } else if self.is_type(v, instance, kind)? {
                return Ok(());
            }
        }
        // `repr(type["name"])` inside `try: ... except Exception`, which a `RecursionError` falls through too.
        let mut reprs = Vec::new();
        for kind in &types {
            let named = kind.get("name").and_then(|name| self.repr_called(name).ok());
            reprs.push(match named {
                Some(text) => text,
                None => self.repr_called(kind)?,
            });
        }
        sink.emit(PyError::with_context(
            format!("{} is not of type {}", self.shown(instance)?, reprs.join(", ")),
            all_errors,
        ))
    }

    /// `_keywords.unevaluatedItems` and `_legacy_keywords.unevaluatedItems_draft2019`.
    fn unevaluated_items(&self, v: &Validator<'_>, legacy: bool, instance: &Value, sink: &mut Sink) -> Flow {
        let Some(array) = self.array_of(v, instance)? else {
            return Ok(());
        };
        let evaluated = if legacy {
            self.evaluated_items_draft2019(v, instance, v.schema)?
        } else {
            self.evaluated_items(v, instance, v.schema)?
        };
        let unevaluated: Vec<&Value> = array
            .iter()
            .enumerate()
            .filter(|(index, _)| !evaluated.contains(index))
            .map(|(_, item)| item)
            .collect();
        if unevaluated.is_empty() {
            return Ok(());
        }
        let (joined, verb) = self.extras_msg(unevaluated)?;
        sink.emit(PyError::new(format!(
            "Unevaluated items are not allowed ({joined} {verb} unexpected)"
        )))
    }

    /// `_keywords.unevaluatedProperties` and `_legacy_keywords.unevaluatedProperties_draft2019`.
    fn unevaluated_properties(
        &self,
        v: &Validator<'_>,
        legacy: bool,
        unevaluated_schema: &Value,
        instance: &Value,
        sink: &mut Sink,
    ) -> Flow {
        let Some(object) = self.object_of(v, instance)? else {
            return Ok(());
        };
        let evaluated = if legacy {
            self.evaluated_keys_draft2019(v, instance, v.schema)?
        } else {
            self.evaluated_keys(v, instance, v.schema)?
        };
        let mut unevaluated = Vec::new();
        for (property, child) in object {
            if evaluated.contains(property) {
                continue;
            }
            let place = Place::both(key(property), key(property));
            let site = if legacy {
                Site::UnevaluatedPropertiesDraft2019
            } else {
                Site::UnevaluatedProperties
            };
            let errors = self.descend_looping(v, child, unevaluated_schema, place, site)?;
            unevaluated.extend(errors.iter().map(|_| property));
        }
        if unevaluated.is_empty() {
            return Ok(());
        }
        let message = if *unevaluated_schema == Value::Bool(false) {
            unevaluated.sort();
            let (joined, verb) = self.extras_msg(&strings(unevaluated))?;
            format!("Unevaluated properties are not allowed ({joined} {verb} unexpected)")
        } else {
            let (joined, verb) = self.extras_msg(&strings(unevaluated))?;
            format!(
                "Unevaluated properties are not valid under the given schema ({joined} {verb} unevaluated and invalid)"
            )
        };
        sink.emit(PyError::new(message))
    }

    /// The `find_evaluated_*` step for a resolved reference: recursion with
    /// `validator.evolve(schema=resolved.contents, _resolver=resolved.resolver)`.
    fn evaluated_through<T>(
        &self,
        v: &Validator<'_>,
        reference: Option<&Value>,
        instance: &Value,
        find: Find<'s, T>,
    ) -> Result<Vec<T>, PythonString> {
        let resolved = match reference {
            Some(reference) => {
                self.peek(self.lookup_frames(v, reference))?;
                v.resolver.lookup(&self.registry, reference)
            }
            None => {
                self.peek(self.recursive_lookup_frames())?;
                v.resolver.lookup_recursive(&self.registry)
            }
        };
        let Resolved { contents, resolver } = resolved.map_err(RefError::raw)?;
        let contents: &Value = &contents;
        let next = self.evolve_with(v, contents, resolver)?;
        find(self, &next, instance, contents)
    }

    /// The `if`/`then`/`else` step of the `find_evaluated_*` helpers.
    fn evaluated_branch<T>(
        &self,
        v: &Validator<'_>,
        instance: &Value,
        schema: &Value,
        find: Find<'s, T>,
    ) -> Result<Vec<T>, PythonString> {
        let mut evaluated = Vec::new();
        if !ops::has(schema, "if")? {
            return Ok(evaluated);
        }
        let checker = self.evolve(v, &schema["if"])?;
        if self.is_valid(&checker, instance)? {
            evaluated.extend(find(self, v, instance, &schema["if"])?);
            if ops::has(schema, "then")? {
                evaluated.extend(find(self, v, instance, &schema["then"])?);
            }
        } else if ops::has(schema, "else")? {
            evaluated.extend(find(self, v, instance, &schema["else"])?);
        }
        Ok(evaluated)
    }

    /// The `allOf`/`oneOf`/`anyOf` step of `find_evaluated_item_indexes_by_schema` (both drafts) and of the
    /// draft 2019-09 `find_evaluated_property_keys_by_schema`: `if keyword in schema`, then
    /// `next(validator.descend(instance, subschema), None) is None`.
    fn evaluated_applicators<T>(
        &self,
        v: &Validator<'_>,
        instance: &Value,
        schema: &Value,
        find: Find<'s, T>,
        site: Site,
    ) -> Result<Vec<T>, PythonString> {
        let mut evaluated = Vec::new();
        for keyword in ["allOf", "oneOf", "anyOf"] {
            if !ops::has(schema, keyword)? {
                continue;
            }
            for subschema in ops::iterate(&schema[keyword])? {
                if self.descend_valid(v, instance, &subschema, site)? {
                    evaluated.extend(find(self, v, instance, &subschema)?);
                }
            }
        }
        Ok(evaluated)
    }

    /// The `contains`/`unevaluatedItems` step of `find_evaluated_item_indexes_by_schema`.
    fn evaluated_matches(
        &self,
        v: &Validator<'_>,
        instance: &Value,
        schema: &Value,
    ) -> Result<Vec<usize>, PythonString> {
        let mut evaluated = Vec::new();
        for keyword in ["contains", "unevaluatedItems"] {
            if !ops::has(schema, keyword)? {
                continue;
            }
            for (index, item) in instance.as_array().into_iter().flatten().enumerate() {
                let checker = self.evolve(v, &schema[keyword])?;
                if self.is_valid(&checker, item)? {
                    evaluated.push(index);
                }
            }
        }
        Ok(evaluated)
    }

    /// `jsonschema._utils.find_evaluated_item_indexes_by_schema` (draft 2020-12).
    fn evaluated_items(&self, v: &Validator<'_>, instance: &Value, schema: &Value) -> Result<Vec<usize>, PythonString> {
        let _depth = self.enter(1)?;
        self.charge_type(v.draft, schema, Some("boolean"))?;
        if schema.is_boolean() {
            return Ok(Vec::new());
        }
        let length = instance.as_array().map_or(0, Vec::len);
        if ops::has(schema, "items")? {
            return Ok((0..length).collect());
        }
        let mut evaluated = Vec::new();
        for keyword in ["$ref", "$dynamicRef"] {
            if let Some(reference) = ops::get_some(schema, keyword)? {
                evaluated.extend(self.evaluated_through(v, Some(reference), instance, Self::evaluated_items)?);
            }
        }
        if ops::has(schema, "prefixItems")? {
            evaluated.extend(0..ops::len(&schema["prefixItems"])?);
        }
        evaluated.extend(self.evaluated_branch(v, instance, schema, Self::evaluated_items)?);
        evaluated.extend(self.evaluated_matches(v, instance, schema)?);
        evaluated.extend(self.evaluated_applicators(
            v,
            instance,
            schema,
            Self::evaluated_items,
            Site::EvaluatedItems,
        )?);
        Ok(evaluated)
    }

    /// `jsonschema._legacy_keywords.find_evaluated_item_indexes_by_schema` (draft 2019-09).
    fn evaluated_items_draft2019(
        &self,
        v: &Validator<'_>,
        instance: &Value,
        schema: &Value,
    ) -> Result<Vec<usize>, PythonString> {
        let _depth = self.enter(1)?;
        self.charge_type(v.draft, schema, Some("boolean"))?;
        if schema.is_boolean() {
            return Ok(Vec::new());
        }
        let length = instance.as_array().map_or(0, Vec::len);
        let mut evaluated = Vec::new();
        if let Some(reference) = ops::get_some(schema, "$ref")? {
            evaluated.extend(self.evaluated_through(v, Some(reference), instance, Self::evaluated_items_draft2019)?);
        }
        if ops::has(schema, "$recursiveRef")? {
            evaluated.extend(self.evaluated_through(v, None, instance, Self::evaluated_items_draft2019)?);
        }
        if ops::has(schema, "items")? {
            if ops::has(schema, "additionalItems")? || self.is_kind(v, &schema["items"], "object")? {
                return Ok((0..length).collect());
            }
            evaluated.extend(0..ops::len(&schema["items"])?);
        }
        evaluated.extend(self.evaluated_branch(v, instance, schema, Self::evaluated_items_draft2019)?);
        evaluated.extend(self.evaluated_matches(v, instance, schema)?);
        evaluated.extend(self.evaluated_applicators(
            v,
            instance,
            schema,
            Self::evaluated_items_draft2019,
            Site::EvaluatedItemsDraft2019,
        )?);
        Ok(evaluated)
    }

    /// The `patternProperties` and `dependentSchemas` steps of both `find_evaluated_property_keys_by_schema`.
    fn evaluated_patterns(
        &self,
        v: &Validator<'_>,
        instance: &Value,
        schema: &Value,
        find: Find<'s, PythonString>,
    ) -> Result<Vec<PythonString>, PythonString> {
        let empty = PythonJsonObject::new();
        let object = instance.as_object().unwrap_or(&empty);
        let mut evaluated = Vec::new();
        if ops::has(schema, "patternProperties")? {
            for property in object.keys() {
                for pattern in ops::iterate(&schema["patternProperties"])? {
                    if self.search(&pattern, property)? {
                        evaluated.push(property.clone());
                    }
                }
            }
        }
        if ops::has(schema, "dependentSchemas")? {
            for (property, subschema) in ops::items(&schema["dependentSchemas"])? {
                if ops::has_name(instance, property)? {
                    evaluated.extend(find(self, v, instance, subschema)?);
                }
            }
        }
        Ok(evaluated)
    }

    /// `jsonschema._utils.find_evaluated_property_keys_by_schema` (draft 2020-12).
    fn evaluated_keys(
        &self,
        v: &Validator<'_>,
        instance: &Value,
        schema: &Value,
    ) -> Result<Vec<PythonString>, PythonString> {
        let _depth = self.enter(1)?;
        self.charge_type(v.draft, schema, Some("boolean"))?;
        if schema.is_boolean() {
            return Ok(Vec::new());
        }
        let empty = PythonJsonObject::new();
        let object = instance.as_object().unwrap_or(&empty);
        let mut evaluated = Vec::new();
        for keyword in ["$ref", "$dynamicRef"] {
            if let Some(reference) = ops::get_some(schema, keyword)? {
                evaluated.extend(self.evaluated_through(v, Some(reference), instance, Self::evaluated_keys)?);
            }
        }
        let properties = ops::get(schema, "properties")?;
        self.charge_type(v.draft, properties.unwrap_or(&Value::Null), Some("object"))?;
        if let Some(Value::Object(properties)) = properties {
            evaluated.extend(object.keys().filter(|name| properties.contains_key(*name)).cloned());
        }
        for keyword in ["additionalProperties", "unevaluatedProperties"] {
            let Some(subschema) = ops::get_some(schema, keyword)? else {
                continue;
            };
            // The generator `+=` drains, resuming it from C, which calls `_utils.is_valid` on each `descend`.
            let _generator = self.enter(1)?;
            let _resumed = self.hold(RESUMED);
            for (property, child) in object {
                let _is_valid = self.enter(1)?;
                if self.descend_valid(v, child, subschema, Site::UtilsIsValid)? {
                    evaluated.push(property.clone());
                }
            }
        }
        evaluated.extend(self.evaluated_patterns(v, instance, schema, Self::evaluated_keys)?);
        for keyword in ["allOf", "oneOf", "anyOf"] {
            let subschemas = match ops::get(schema, keyword)? {
                Some(subschemas) => ops::iterate(subschemas)?,
                None => Vec::new(),
            };
            for subschema in subschemas {
                let valid = {
                    // `_utils.is_valid`, which drains the `descend`.
                    let _is_valid = self.enter(1)?;
                    self.descend_valid(v, instance, &subschema, Site::UtilsIsValid)?
                };
                if valid {
                    evaluated.extend(self.evaluated_keys(v, instance, &subschema)?);
                }
            }
        }
        evaluated.extend(self.evaluated_branch(v, instance, schema, Self::evaluated_keys)?);
        Ok(evaluated)
    }

    /// `jsonschema._legacy_keywords.find_evaluated_property_keys_by_schema` (draft 2019-09).
    fn evaluated_keys_draft2019(
        &self,
        v: &Validator<'_>,
        instance: &Value,
        schema: &Value,
    ) -> Result<Vec<PythonString>, PythonString> {
        let _depth = self.enter(1)?;
        self.charge_type(v.draft, schema, Some("boolean"))?;
        if schema.is_boolean() {
            return Ok(Vec::new());
        }
        let empty = PythonJsonObject::new();
        let object = instance.as_object().unwrap_or(&empty);
        let mut evaluated = Vec::new();
        if let Some(reference) = ops::get_some(schema, "$ref")? {
            evaluated.extend(self.evaluated_through(v, Some(reference), instance, Self::evaluated_keys_draft2019)?);
        }
        if ops::has(schema, "$recursiveRef")? {
            evaluated.extend(self.evaluated_through(v, None, instance, Self::evaluated_keys_draft2019)?);
        }
        for keyword in ["properties", "additionalProperties", "unevaluatedProperties"] {
            if !ops::has(schema, keyword)? {
                continue;
            }
            let value = &schema[keyword];
            if self.is_kind(v, value, "boolean")? {
                if *value == Value::Bool(true) {
                    evaluated.extend(object.keys().cloned());
                }
            } else if self.is_kind(v, value, "object")?
                && let Value::Object(subschemas) = value
            {
                evaluated.extend(subschemas.keys().filter(|name| object.contains_key(*name)).cloned());
            }
        }
        evaluated.extend(self.evaluated_patterns(v, instance, schema, Self::evaluated_keys_draft2019)?);
        evaluated.extend(self.evaluated_applicators(
            v,
            instance,
            schema,
            Self::evaluated_keys_draft2019,
            Site::EvaluatedKeysDraft2019,
        )?);
        evaluated.extend(self.evaluated_branch(v, instance, schema, Self::evaluated_keys_draft2019)?);
        Ok(evaluated)
    }
}

impl Engine<'_> {
    /// `_keywords.type`: `any(validator.is_type(instance, type) for type in ensure_list(types))`.
    fn type_keyword(&self, v: &Validator<'_>, types: &Value, instance: &Value, sink: &mut Sink) -> Flow {
        // `ensure_list`.
        self.peek(1)?;
        let types = ensure_list(types)?;
        let matched = {
            let _generator = self.enter(1)?;
            let mut matched = false;
            for kind in &types {
                if self.is_type(v, instance, kind)? {
                    matched = true;
                    break;
                }
            }
            matched
        };
        if matched {
            return Ok(());
        }
        sink.emit(PyError::new(format!(
            "{} is not of type {}",
            self.shown(instance)?,
            self.join_reprs(types.iter().map(|kind| &**kind))?
        )))
    }
}
