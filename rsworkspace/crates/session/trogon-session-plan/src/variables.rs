use crate::constants::SAFE_INTEGER;
use crate::wire::agent::variable_type::Kind;
use crate::{PlanError, wire::agent};
use buffa_types::google::protobuf::{Struct, Value, value};
use std::collections::BTreeSet;

pub(crate) fn shape(bindings: &agent::VariableBindings) -> Result<(), PlanError> {
    let values = bindings.values.as_option().ok_or(PlanError::Variables)?;
    let mut remaining = 512usize;
    object_shape(values, 0, &mut remaining)
}

fn spend(depth: usize, remaining: &mut usize) -> Result<(), PlanError> {
    if depth > 8 {
        return Err(PlanError::Limit);
    }
    *remaining = remaining.checked_sub(1).ok_or(PlanError::Limit)?;
    Ok(())
}

fn object_shape(object: &Struct, depth: usize, remaining: &mut usize) -> Result<(), PlanError> {
    spend(depth, remaining)?;
    if object.fields.len() > 128 {
        return Err(PlanError::Limit);
    }
    for (name, value) in &object.fields {
        spend(depth, remaining)?;
        if name.is_empty() || name.len() > 128 {
            return Err(PlanError::Variables);
        }
        value_shape(value, depth + 1, remaining)?;
    }
    Ok(())
}

fn value_shape(value: &Value, depth: usize, remaining: &mut usize) -> Result<(), PlanError> {
    spend(depth, remaining)?;
    match value.kind.as_ref().ok_or(PlanError::Variables)? {
        value::Kind::NullValue(_) => Err(PlanError::Variables),
        value::Kind::StringValue(text) if text.len() > 4096 => Err(PlanError::Limit),
        value::Kind::NumberValue(number) if !number.is_finite() => Err(PlanError::Variables),
        value::Kind::StructValue(object) => object_shape(object, depth, remaining),
        value::Kind::ListValue(list) => {
            for item in &list.values {
                value_shape(item, depth + 1, remaining)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

pub(crate) fn verify(
    schema: Option<&agent::VariableSchema>,
    bindings: &agent::VariableBindings,
) -> Result<(), PlanError> {
    shape(bindings)?;
    let definitions = schema.map(|schema| schema.variables.as_slice()).unwrap_or_default();
    let mut remaining = 512usize;
    schema_shape(definitions, 0, &mut remaining)?;
    let values = bindings.values.as_option().ok_or(PlanError::Variables)?;
    match_object(definitions, values)
}

fn schema_shape(
    definitions: &[agent::VariableDefinition],
    depth: usize,
    remaining: &mut usize,
) -> Result<(), PlanError> {
    spend(depth, remaining)?;
    if definitions.len() > 128 {
        return Err(PlanError::Limit);
    }
    let mut names = BTreeSet::new();
    for definition in definitions {
        spend(depth, remaining)?;
        if definition.name.is_empty()
            || definition.name.len() > 128
            || !names.insert(&definition.name)
            || !matches!(
                definition.requirement.as_known(),
                Some(agent::VariableRequirement::Required | agent::VariableRequirement::Optional)
            )
        {
            return Err(PlanError::Variables);
        }
        type_shape(
            definition.value_type.as_option().ok_or(PlanError::Variables)?,
            depth + 1,
            remaining,
        )?;
    }
    Ok(())
}

fn type_shape(ty: &agent::VariableType, depth: usize, remaining: &mut usize) -> Result<(), PlanError> {
    spend(depth, remaining)?;
    match ty.kind.as_ref().ok_or(PlanError::Variables)? {
        Kind::StringType(ty) => {
            if ty.max_length.is_some_and(|max| ty.min_length.unwrap_or_default() > max) || ty.allowed_values.len() > 128
            {
                return Err(PlanError::Variables);
            }
            let mut choices = BTreeSet::new();
            for choice in &ty.allowed_values {
                if choice.len() > 4096 || !choices.insert(choice) || !string_matches(ty, choice) {
                    return Err(PlanError::Variables);
                }
            }
        }
        Kind::BooleanType(_) => {}
        Kind::IntegerType(ty) => {
            let minimum = ty.minimum.unwrap_or(-SAFE_INTEGER);
            let maximum = ty.maximum.unwrap_or(SAFE_INTEGER);
            if minimum < -SAFE_INTEGER || maximum > SAFE_INTEGER || minimum > maximum {
                return Err(PlanError::Variables);
            }
        }
        Kind::NumberType(ty) => {
            if ty.minimum.is_some_and(|value| !value.is_finite())
                || ty.maximum.is_some_and(|value| !value.is_finite())
                || ty
                    .minimum
                    .zip(ty.maximum)
                    .is_some_and(|(minimum, maximum)| minimum > maximum)
            {
                return Err(PlanError::Variables);
            }
        }
        Kind::ListType(ty) => {
            if ty
                .max_items
                .is_some_and(|maximum| ty.min_items.unwrap_or_default() > maximum)
            {
                return Err(PlanError::Variables);
            }
            type_shape(
                ty.element_type.as_option().ok_or(PlanError::Variables)?,
                depth + 1,
                remaining,
            )?;
        }
        Kind::ObjectType(ty) => schema_shape(&ty.fields, depth, remaining)?,
    }
    Ok(())
}

fn string_matches(ty: &agent::StringVariableType, text: &str) -> bool {
    let length = text.chars().count();
    length >= ty.min_length.unwrap_or_default() as usize
        && ty.max_length.is_none_or(|max| length <= max as usize)
        && (ty.allowed_values.is_empty() || ty.allowed_values.iter().any(|choice| choice == text))
}

fn match_object(definitions: &[agent::VariableDefinition], values: &Struct) -> Result<(), PlanError> {
    if values
        .fields
        .keys()
        .any(|name| !definitions.iter().any(|definition| &definition.name == name))
    {
        return Err(PlanError::Variables);
    }
    for definition in definitions {
        match values.fields.get(&definition.name) {
            Some(value) => match_value(definition.value_type.as_option().ok_or(PlanError::Variables)?, value)?,
            None if definition.requirement.as_known() == Some(agent::VariableRequirement::Required) => {
                return Err(PlanError::Variables);
            }
            None => {}
        }
    }
    Ok(())
}

fn match_value(ty: &agent::VariableType, value: &Value) -> Result<(), PlanError> {
    let valid = match (ty.kind.as_ref(), value.kind.as_ref()) {
        (Some(Kind::StringType(ty)), Some(value::Kind::StringValue(text))) => string_matches(ty, text),
        (Some(Kind::BooleanType(_)), Some(value::Kind::BoolValue(_))) => true,
        (Some(Kind::IntegerType(ty)), Some(value::Kind::NumberValue(number))) => {
            number.is_finite()
                && number.fract() == 0.0
                && *number >= ty.minimum.unwrap_or(-SAFE_INTEGER) as f64
                && *number <= ty.maximum.unwrap_or(SAFE_INTEGER) as f64
        }
        (Some(Kind::NumberType(ty)), Some(value::Kind::NumberValue(number))) => {
            number.is_finite()
                && ty.minimum.is_none_or(|minimum| *number >= minimum)
                && ty.maximum.is_none_or(|maximum| *number <= maximum)
        }
        (Some(Kind::ListType(ty)), Some(value::Kind::ListValue(list))) => {
            if list.values.len() < ty.min_items.unwrap_or_default() as usize
                || ty.max_items.is_some_and(|maximum| list.values.len() > maximum as usize)
            {
                return Err(PlanError::Variables);
            }
            let item_type = ty.element_type.as_option().ok_or(PlanError::Variables)?;
            for value in &list.values {
                match_value(item_type, value)?;
            }
            true
        }
        (Some(Kind::ObjectType(ty)), Some(value::Kind::StructValue(object))) => {
            match_object(&ty.fields, object)?;
            true
        }
        _ => false,
    };
    if !valid {
        return Err(PlanError::Variables);
    }
    Ok(())
}
