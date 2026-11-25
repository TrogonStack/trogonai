use super::{Query, expr};
use crate::Error;
use pg_query::{NodeEnum, protobuf};

fn select() -> Result<protobuf::SelectStmt, Box<dyn std::error::Error>> {
    let parsed = pg_query::parse("SELECT id FROM public.customers ORDER BY id")?;
    match parsed
        .protobuf
        .stmts
        .into_iter()
        .next()
        .and_then(|s| s.stmt)
        .and_then(|s| s.node)
    {
        Some(NodeEnum::SelectStmt(stmt)) => Ok(*stmt),
        _ => Err("expected SELECT fixture".into()),
    }
}

fn wrapped(node: NodeEnum) -> protobuf::Node {
    protobuf::Node { node: Some(node) }
}

#[test]
fn rejects_unexpected_projection_and_sort_ast_shapes() -> Result<(), Box<dyn std::error::Error>> {
    let mut statement = select()?;
    statement.target_list = vec![wrapped(NodeEnum::Integer(protobuf::Integer { ival: 1 }))];
    assert!(matches!(Query::from_statement(&statement), Err(Error::UnsupportedSql)));

    let mut statement = select()?;
    statement.target_list = vec![wrapped(NodeEnum::ResTarget(Box::new(protobuf::ResTarget {
        indirection: vec![wrapped(NodeEnum::Integer(protobuf::Integer { ival: 1 }))],
        ..Default::default()
    })))];
    assert!(matches!(Query::from_statement(&statement), Err(Error::UnsupportedSql)));

    let mut statement = select()?;
    statement.sort_clause = vec![wrapped(NodeEnum::Integer(protobuf::Integer { ival: 1 }))];
    assert!(matches!(Query::from_statement(&statement), Err(Error::UnsupportedSql)));

    let mut statement = select()?;
    statement.sort_clause = vec![wrapped(NodeEnum::SortBy(Box::new(protobuf::SortBy {
        sortby_dir: i32::MAX,
        sortby_nulls: protobuf::SortByNulls::SortbyNullsDefault as i32,
        ..Default::default()
    })))];
    assert!(matches!(Query::from_statement(&statement), Err(Error::UnsupportedSql)));
    Ok(())
}

#[test]
fn rejects_invalid_relation_aliases_from_the_parser_boundary() -> Result<(), Box<dyn std::error::Error>> {
    for alias in [String::new(), "x".repeat(64), "a\0b".into()] {
        let mut statement = select()?;
        let Some(NodeEnum::RangeVar(table)) = statement.from_clause[0].node.as_mut() else {
            return Err("expected table fixture".into());
        };
        table.alias = Some(protobuf::Alias {
            aliasname: alias,
            ..Default::default()
        });
        assert!(matches!(Query::from_statement(&statement), Err(Error::Identifier)));
    }
    Ok(())
}

#[test]
fn rejects_incomplete_boolean_and_missing_ast_nodes() {
    let incomplete_boolean = wrapped(NodeEnum::BoolExpr(Box::new(protobuf::BoolExpr {
        boolop: protobuf::BoolExprType::AndExpr as i32,
        args: Vec::new(),
        ..Default::default()
    })));
    assert!(matches!(expr(&incomplete_boolean, 0), Err(Error::UnsupportedSql)));
    assert!(matches!(
        expr(&protobuf::Node::default(), 0),
        Err(Error::UnsupportedSql)
    ));
}
