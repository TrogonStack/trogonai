#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use cel_parser::ast::{IdedEntryExpr, MapEntryExpr, MapExpr, StructFieldExpr};

use super::*;

fn expression(expr: Expr) -> Expression {
    Expression { id: 0, expr }
}

fn name() -> Expression {
    expression(Expr::Ident("a".to_owned()))
}

fn map_of(entry: EntryExpr) -> Expression {
    expression(Expr::Map(MapExpr {
        entries: vec![IdedEntryExpr { id: 0, expr: entry }],
    }))
}

fn refused(expression: &Expression) -> CompileError {
    static_cost(expression, &CelLimits::default()).expect_err("the interpreter cannot run it")
}

// The parser never builds these three, so only a hand-built tree reaches the
// guards that keep a future parser from slipping them past the cost model.

#[test]
fn an_unspecified_expression_is_refused() {
    assert_eq!(
        refused(&expression(Expr::Unspecified)),
        CompileError::Unsupported {
            construct: "an unspecified expression"
        }
    );
}

#[test]
fn an_optional_map_entry_is_refused() {
    let optional = EntryExpr::MapEntry(MapEntryExpr {
        key: name(),
        value: name(),
        optional: true,
    });
    assert_eq!(
        refused(&map_of(optional)),
        CompileError::Unsupported {
            construct: "an optional map entry"
        }
    );
}

#[test]
fn a_struct_field_inside_a_map_is_refused() {
    let field = EntryExpr::StructField(StructFieldExpr {
        field: "a".to_owned(),
        value: name(),
        optional: false,
    });
    assert_eq!(
        refused(&map_of(field)),
        CompileError::Unsupported {
            construct: "a struct field"
        }
    );
}
