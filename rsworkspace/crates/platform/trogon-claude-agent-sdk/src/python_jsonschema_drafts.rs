//! The validator classes `jsonschema.validators.validator_for` chooses between (`Draft3Validator` through
//! `Draft202012Validator`): each one's `VALIDATORS` table, `TYPE_CHECKER`, `applicable_validators` and the
//! `referencing.jsonschema` specification its resources use.

use super::ops;
use crate::constants::{
    JSONSCHEMA_DRAFTS_DRAFT7_IN_VALUE, JSONSCHEMA_DRAFTS_DRAFT201909_IN_VALUE, JSONSCHEMA_DRAFTS_DRAFT202012_IN_VALUE,
};
use crate::{
    python_json_value::PythonJsonValue as Value, python_string::PythonString, python_urllib::geturl,
    python_value_repr::type_name,
};
use Draft::{Draft3, Draft4, Draft6, Draft7, Draft201909, Draft202012};

/// One `jsonschema` validator class, ordered from the oldest draft to the newest.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) enum Draft {
    Draft3,
    Draft4,
    Draft6,
    Draft7,
    Draft201909,
    Draft202012,
}

/// The keyword functions of `jsonschema._keywords` and `jsonschema._legacy_keywords`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Keyword {
    AdditionalItems,
    AdditionalProperties,
    AllOf,
    AnyOf,
    Const,
    Contains,
    ContainsDraft6Draft7,
    DependenciesDraft3,
    DependenciesDraft4Draft6Draft7,
    DependentRequired,
    DependentSchemas,
    DisallowDraft3,
    DynamicRef,
    Enum,
    ExclusiveMaximum,
    ExclusiveMinimum,
    ExtendsDraft3,
    Format,
    If,
    Items,
    ItemsDraft3Draft4,
    ItemsDraft6Draft7Draft201909,
    MaxItems,
    MaxLength,
    MaxProperties,
    Maximum,
    MaximumDraft3Draft4,
    MinItems,
    MinLength,
    MinProperties,
    Minimum,
    MinimumDraft3Draft4,
    MultipleOf,
    Not,
    OneOf,
    Pattern,
    PatternProperties,
    PrefixItems,
    Properties,
    PropertiesDraft3,
    PropertyNames,
    RecursiveRef,
    Ref,
    Required,
    Type,
    TypeDraft3,
    UnevaluatedItems,
    UnevaluatedItemsDraft2019,
    UnevaluatedProperties,
    UnevaluatedPropertiesDraft2019,
    UniqueItems,
}

/// `id_of` of a `referencing.jsonschema` specification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum IdOf {
    /// `_dollar_id`.
    Dollar,
    /// `_legacy_dollar_id`.
    LegacyDollar,
    /// `_legacy_id`.
    Legacy,
}

/// `anchors_in` of a `referencing.jsonschema` specification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum AnchorsIn {
    /// `_anchor`: `$anchor` and `$dynamicAnchor`.
    Dollar,
    /// `_anchor_2019`: `$anchor` only.
    Dollar2019,
    /// `_legacy_anchor_in_dollar_id`.
    LegacyDollarId,
    /// `_legacy_anchor_in_id`.
    LegacyId,
}

/// Which `subresources_of` and `maybe_in_subresource` variant a specification uses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Crazy {
    /// `_subresources_of` and `_maybe_in_subresource`.
    Plain,
    /// `_subresources_of_with_crazy_items` and `_maybe_in_subresource_crazy_items`.
    Items,
    /// `_subresources_of_with_crazy_items_dependencies` and `_maybe_in_subresource_crazy_items_dependencies`.
    ItemsDependencies,
    /// `_subresources_of_with_crazy_aP_items_dependencies` and `_maybe_in_subresource_crazy_items_dependencies`.
    AdditionalItemsDependencies,
}

/// A `referencing.jsonschema` specification. `in_value`, `in_subarray` and `in_subvalues` are the set displays
/// `subresources_of` iterates, which iterate in an order that depends on the hash secret; each is listed in the order
/// [`crate::python_set::display_order`] takes, `sorted(keys, key=marshal.dumps)` for three or more keys (the one key
/// that is not an identifier, `$defs`, is not interned, so its type code sorts first, then the length, then the
/// characters). The `maybe_in` lists only answer membership.
#[derive(Clone, Copy, Debug)]
pub(super) struct Specification {
    pub(super) id_of: IdOf,
    pub(super) anchors_in: AnchorsIn,
    pub(super) crazy: Crazy,
    pub(super) in_value: &'static [&'static str],
    pub(super) in_subarray: &'static [&'static str],
    pub(super) in_subvalues: &'static [&'static str],
    /// `in_value` of `maybe_in_subresource`.
    pub(super) maybe_in_value: &'static [&'static str],
    /// `in_subvalues | in_subarray` of `maybe_in_subresource`.
    pub(super) maybe_in_child: &'static [&'static str],
}

impl Specification {
    const DRAFT3: Self = Self {
        id_of: IdOf::Legacy,
        anchors_in: AnchorsIn::LegacyId,
        crazy: Crazy::AdditionalItemsDependencies,
        in_value: &[],
        in_subarray: &["extends"],
        in_subvalues: &["properties", "definitions", "patternProperties"],
        maybe_in_value: &["additionalItems", "additionalProperties"],
        maybe_in_child: &["definitions", "extends", "patternProperties", "properties"],
    };

    const DRAFT4: Self = Self {
        id_of: IdOf::Legacy,
        anchors_in: AnchorsIn::LegacyId,
        crazy: Crazy::AdditionalItemsDependencies,
        in_value: &["not"],
        in_subarray: &["allOf", "anyOf", "oneOf"],
        in_subvalues: &["properties", "definitions", "patternProperties"],
        maybe_in_value: &["additionalItems", "additionalProperties", "not"],
        maybe_in_child: &[
            "allOf",
            "anyOf",
            "definitions",
            "oneOf",
            "patternProperties",
            "properties",
        ],
    };

    const DRAFT6: Self = Self {
        id_of: IdOf::LegacyDollar,
        anchors_in: AnchorsIn::LegacyDollarId,
        crazy: Crazy::ItemsDependencies,
        in_value: &[
            "not",
            "contains",
            "propertyNames",
            "additionalItems",
            "additionalProperties",
        ],
        in_subarray: &["allOf", "anyOf", "oneOf"],
        in_subvalues: &["properties", "definitions", "patternProperties"],
        maybe_in_value: &[
            "additionalItems",
            "additionalProperties",
            "contains",
            "not",
            "propertyNames",
        ],
        maybe_in_child: &[
            "allOf",
            "anyOf",
            "definitions",
            "oneOf",
            "patternProperties",
            "properties",
        ],
    };

    const DRAFT7: Self = Self {
        id_of: IdOf::LegacyDollar,
        anchors_in: AnchorsIn::LegacyDollarId,
        crazy: Crazy::ItemsDependencies,
        in_value: JSONSCHEMA_DRAFTS_DRAFT7_IN_VALUE,
        in_subarray: &["allOf", "anyOf", "oneOf"],
        in_subvalues: &["properties", "definitions", "patternProperties"],
        maybe_in_value: JSONSCHEMA_DRAFTS_DRAFT7_IN_VALUE,
        maybe_in_child: &[
            "allOf",
            "anyOf",
            "definitions",
            "oneOf",
            "patternProperties",
            "properties",
        ],
    };

    const DRAFT201909: Self = Self {
        id_of: IdOf::Dollar,
        anchors_in: AnchorsIn::Dollar2019,
        crazy: Crazy::Items,
        in_value: JSONSCHEMA_DRAFTS_DRAFT201909_IN_VALUE,
        in_subarray: &["allOf", "anyOf", "oneOf"],
        in_subvalues: &[
            "$defs",
            "properties",
            "definitions",
            "dependentSchemas",
            "patternProperties",
        ],
        maybe_in_value: JSONSCHEMA_DRAFTS_DRAFT201909_IN_VALUE,
        maybe_in_child: &[
            "$defs",
            "allOf",
            "anyOf",
            "definitions",
            "dependentSchemas",
            "oneOf",
            "patternProperties",
            "properties",
        ],
    };

    const DRAFT202012: Self = Self {
        id_of: IdOf::Dollar,
        anchors_in: AnchorsIn::Dollar,
        crazy: Crazy::Plain,
        in_value: JSONSCHEMA_DRAFTS_DRAFT202012_IN_VALUE,
        in_subarray: &["allOf", "anyOf", "oneOf", "prefixItems"],
        in_subvalues: &[
            "$defs",
            "properties",
            "definitions",
            "dependentSchemas",
            "patternProperties",
        ],
        maybe_in_value: JSONSCHEMA_DRAFTS_DRAFT202012_IN_VALUE,
        maybe_in_child: &[
            "$defs",
            "allOf",
            "anyOf",
            "definitions",
            "dependentSchemas",
            "oneOf",
            "patternProperties",
            "prefixItems",
            "properties",
        ],
    };
}

impl Draft {
    const ALL: [Self; 6] = [
        Self::Draft3,
        Self::Draft4,
        Self::Draft6,
        Self::Draft7,
        Self::Draft201909,
        Self::Draft202012,
    ];

    /// The metaschema id without its empty fragment: the key `_META_SCHEMAS` normalizes it to and the key of
    /// `referencing.jsonschema`'s specifications.
    pub(super) fn dialect(self) -> &'static str {
        match self {
            Self::Draft3 => "http://json-schema.org/draft-03/schema",
            Self::Draft4 => "http://json-schema.org/draft-04/schema",
            Self::Draft6 => "http://json-schema.org/draft-06/schema",
            Self::Draft7 => "http://json-schema.org/draft-07/schema",
            Self::Draft201909 => "https://json-schema.org/draft/2019-09/schema",
            Self::Draft202012 => "https://json-schema.org/draft/2020-12/schema",
        }
    }

    /// The class whose dialect is exactly `id`.
    pub(super) fn with_dialect(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|draft| draft.dialect() == id)
    }

    pub(super) fn specification(self) -> &'static Specification {
        match self {
            Self::Draft3 => &Specification::DRAFT3,
            Self::Draft4 => &Specification::DRAFT4,
            Self::Draft6 => &Specification::DRAFT6,
            Self::Draft7 => &Specification::DRAFT7,
            Self::Draft201909 => &Specification::DRAFT201909,
            Self::Draft202012 => &Specification::DRAFT202012,
        }
    }

    /// `applicable_validators` is `_legacy_keywords.ignore_ref_siblings` up to draft 7 and `methodcaller("items")`
    /// after it.
    pub(super) fn ignores_ref_siblings(self) -> bool {
        self <= Self::Draft7
    }

    /// `VALIDATORS.get(name)`.
    pub(super) fn keyword(self, name: &str) -> Option<Keyword> {
        let draft = self;
        let keyword = match name {
            "$ref" => Keyword::Ref,
            "$dynamicRef" if draft == Draft202012 => Keyword::DynamicRef,
            "$recursiveRef" if draft == Draft201909 => Keyword::RecursiveRef,
            "additionalItems" if draft != Draft202012 => Keyword::AdditionalItems,
            "additionalProperties" => Keyword::AdditionalProperties,
            "allOf" if draft != Draft3 => Keyword::AllOf,
            "anyOf" if draft != Draft3 => Keyword::AnyOf,
            "const" if draft >= Draft6 => Keyword::Const,
            "contains" if draft >= Draft201909 => Keyword::Contains,
            "contains" if draft >= Draft6 => Keyword::ContainsDraft6Draft7,
            "dependencies" if draft == Draft3 => Keyword::DependenciesDraft3,
            "dependencies" if draft <= Draft7 => Keyword::DependenciesDraft4Draft6Draft7,
            "dependentRequired" if draft >= Draft201909 => Keyword::DependentRequired,
            "dependentSchemas" if draft >= Draft201909 => Keyword::DependentSchemas,
            "disallow" if draft == Draft3 => Keyword::DisallowDraft3,
            "divisibleBy" if draft == Draft3 => Keyword::MultipleOf,
            "enum" => Keyword::Enum,
            "exclusiveMaximum" if draft >= Draft6 => Keyword::ExclusiveMaximum,
            "exclusiveMinimum" if draft >= Draft6 => Keyword::ExclusiveMinimum,
            "extends" if draft == Draft3 => Keyword::ExtendsDraft3,
            "format" => Keyword::Format,
            "if" if draft >= Draft7 => Keyword::If,
            "items" if draft == Draft202012 => Keyword::Items,
            "items" if draft >= Draft6 => Keyword::ItemsDraft6Draft7Draft201909,
            "items" => Keyword::ItemsDraft3Draft4,
            "maxItems" => Keyword::MaxItems,
            "maxLength" => Keyword::MaxLength,
            "maxProperties" if draft != Draft3 => Keyword::MaxProperties,
            "maximum" if draft <= Draft4 => Keyword::MaximumDraft3Draft4,
            "maximum" => Keyword::Maximum,
            "minItems" => Keyword::MinItems,
            "minLength" => Keyword::MinLength,
            "minProperties" if draft != Draft3 => Keyword::MinProperties,
            "minimum" if draft <= Draft4 => Keyword::MinimumDraft3Draft4,
            "minimum" => Keyword::Minimum,
            "multipleOf" if draft != Draft3 => Keyword::MultipleOf,
            "not" if draft != Draft3 => Keyword::Not,
            "oneOf" if draft != Draft3 => Keyword::OneOf,
            "pattern" => Keyword::Pattern,
            "patternProperties" => Keyword::PatternProperties,
            "prefixItems" if draft == Draft202012 => Keyword::PrefixItems,
            "properties" if draft == Draft3 => Keyword::PropertiesDraft3,
            "properties" => Keyword::Properties,
            "propertyNames" if draft >= Draft6 => Keyword::PropertyNames,
            "required" if draft != Draft3 => Keyword::Required,
            "type" if draft == Draft3 => Keyword::TypeDraft3,
            "type" => Keyword::Type,
            "unevaluatedItems" if draft == Draft201909 => Keyword::UnevaluatedItemsDraft2019,
            "unevaluatedItems" if draft == Draft202012 => Keyword::UnevaluatedItems,
            "unevaluatedProperties" if draft == Draft201909 => Keyword::UnevaluatedPropertiesDraft2019,
            "unevaluatedProperties" if draft == Draft202012 => Keyword::UnevaluatedProperties,
            "uniqueItems" => Keyword::UniqueItems,
            _ => return None,
        };
        Some(keyword)
    }

    /// `TYPE_CHECKER.is_type(instance, name)`, `None` where the checker raises `UndefinedTypeCheck`.
    pub(super) fn type_check(self, instance: &Value, name: &str) -> Option<bool> {
        Some(match name {
            "array" => instance.is_array(),
            "boolean" => instance.is_boolean(),
            "null" => instance.is_null(),
            "number" => instance.is_number(),
            "object" => instance.is_object(),
            "string" => instance.is_string(),
            "integer" => match instance {
                Value::Number(number) if number.is_f64() => self >= Self::Draft6 && number.to_f64().fract() == 0.0,
                other => other.is_number(),
            },
            "any" if self == Self::Draft3 => true,
            _ => return None,
        })
    }
}

/// `Resource.from_contents(contents, default_specification=default)`: a mapping's `$schema`, stripped of `#`,
/// picks the specification when it names one exactly.
pub(super) fn detect(contents: &Value, default: Draft) -> Result<Draft, String> {
    let Value::Object(map) = contents else {
        return Ok(default);
    };
    match map.get("$schema") {
        None | Some(Value::Null) => Ok(default),
        // Every dialect is ASCII, so an id holding a lone surrogate names none.
        Some(Value::String(id)) => Ok(id
            .as_str()
            .and_then(|id| Draft::with_dialect(id.trim_end_matches('#')))
            .unwrap_or(default)),
        Some(other) => Err(format!("'{}' object has no attribute 'rstrip'", type_name(other))),
    }
}

/// `jsonschema.validators.validator_for(schema, default=default)`; `_META_SCHEMAS` is a `URIDict`, so the
/// `$schema` value is normalized with `urlsplit(uri).geturl()` before the lookup. A `$schema` holding a lone
/// surrogate is normalized like any other and names no draft; the `ValueError` its netloc raises names it as is.
pub(super) fn validator_for(schema: &Value, default: Draft) -> Result<Draft, PythonString> {
    if schema.is_boolean() || !ops::has(schema, "$schema")? {
        return Ok(default);
    }
    let null = Value::Null;
    let declared = match schema {
        Value::Object(map) => map.get("$schema").unwrap_or(&null),
        Value::String(_) => return Err("string indices must be integers, not 'str'".into()),
        _ => return Err("list indices must be integers or slices, not str".into()),
    };
    match declared {
        Value::String(uri) => Ok(geturl(uri)?.as_str().and_then(Draft::with_dialect).unwrap_or(default)),
        Value::Array(_) | Value::Object(_) => Err(format!("unhashable type: '{}'", type_name(declared)).into()),
        other if ops::truthy(other) => Err(format!("'{}' object has no attribute 'decode'", type_name(other)).into()),
        _ => Ok(default),
    }
}
