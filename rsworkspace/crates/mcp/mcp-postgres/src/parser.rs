use crate::{Error, Identifier};
use pg_query::{
    NodeEnum,
    protobuf::{self, a_const},
};

#[derive(Debug)]
pub struct Query {
    pub(crate) select: Vec<(Column, Option<Identifier>)>,
    pub(crate) tables: Vec<Table>,
    pub(crate) from: String,
    pub(crate) predicates: Vec<Expr>,
    pub(crate) order: Vec<(Column, &'static str)>,
    pub(crate) limit: crate::row_limit::RowLimit,
}
#[derive(Debug, Clone)]
pub(crate) struct Table {
    pub schema: Identifier,
    pub name: Identifier,
    pub alias: Identifier,
}
#[derive(Debug, Clone)]
pub(crate) struct Column {
    pub qualifier: Option<Identifier>,
    pub name: Identifier,
}
#[derive(Debug)]
pub(crate) enum Expr {
    Column(Column),
    Literal(String),
    Parameter(usize),
    Compare(Box<Expr>, String, Box<Expr>),
    Boolean(Vec<Expr>, &'static str),
}
impl Query {
    pub fn parse(sql: &str) -> Result<Self, Error> {
        if sql.len() > 16_384 {
            return Err(Error::Limit);
        }
        let parsed = pg_query::parse(sql)?;
        if parsed.protobuf.stmts.len() != 1 {
            return Err(Error::UnsupportedSql);
        }
        let statement = parsed
            .protobuf
            .stmts
            .first()
            .and_then(|s| s.stmt.as_ref())
            .ok_or(Error::UnsupportedSql)?;
        let NodeEnum::SelectStmt(stmt) = node(statement)? else {
            return Err(Error::UnsupportedSql);
        };
        Self::from_statement(stmt)
    }
    fn from_statement(stmt: &protobuf::SelectStmt) -> Result<Self, Error> {
        let mut normalized = stmt.clone();
        normalized.target_list.clear();
        normalized.from_clause.clear();
        normalized.where_clause = None;
        normalized.sort_clause.clear();
        normalized.limit_count = None;
        normalized.limit_option = 0;
        normalized.op = 0;
        if normalized != protobuf::SelectStmt::default()
            || stmt.op != protobuf::SetOperation::SetopNone as i32
            || stmt.from_clause.len() != 1
            || stmt.target_list.is_empty()
            || stmt.target_list.len() > 64
        {
            return Err(Error::UnsupportedSql);
        }
        if stmt.limit_option != protobuf::LimitOption::Default as i32
            && stmt.limit_option != protobuf::LimitOption::Count as i32
        {
            return Err(Error::UnsupportedSql);
        }
        let mut query = Self {
            select: Vec::new(),
            tables: Vec::new(),
            from: String::new(),
            predicates: Vec::new(),
            order: Vec::new(),
            limit: crate::row_limit::RowLimit::bounded_default(),
        };
        query.from = query.render_from_node(&stmt.from_clause[0], 0)?;
        for target in &stmt.target_list {
            let NodeEnum::ResTarget(target) = node(target)? else {
                return Err(Error::UnsupportedSql);
            };
            if !target.indirection.is_empty() {
                return Err(Error::UnsupportedSql);
            }
            query.select.push((
                column(target.val.as_deref().ok_or(Error::UnsupportedSql)?)?,
                if target.name.is_empty() {
                    None
                } else {
                    Some(Identifier::new(target.name.clone())?)
                },
            ));
        }
        if let Some(predicate) = stmt.where_clause.as_deref() {
            query.predicates.push(expr(predicate, 0)?);
        }
        for sort in &stmt.sort_clause {
            let NodeEnum::SortBy(sort) = node(sort)? else {
                return Err(Error::UnsupportedSql);
            };
            if !sort.use_op.is_empty() || sort.sortby_nulls != protobuf::SortByNulls::SortbyNullsDefault as i32 {
                return Err(Error::UnsupportedSql);
            }
            let direction = match protobuf::SortByDir::try_from(sort.sortby_dir) {
                Ok(protobuf::SortByDir::SortbyDefault | protobuf::SortByDir::SortbyAsc) => "ASC",
                Ok(protobuf::SortByDir::SortbyDesc) => "DESC",
                _ => return Err(Error::UnsupportedSql),
            };
            query
                .order
                .push((column(sort.node.as_deref().ok_or(Error::UnsupportedSql)?)?, direction));
        }
        if let Some(limit) = stmt.limit_count.as_deref() {
            let NodeEnum::AConst(value) = node(limit)? else {
                return Err(Error::UnsupportedSql);
            };
            let Some(a_const::Val::Ival(value)) = value.val.as_ref() else {
                return Err(Error::UnsupportedSql);
            };
            query.limit =
                crate::row_limit::RowLimit::requested(usize::try_from(value.ival).map_err(|_| Error::UnsupportedSql)?)?;
        }
        Ok(query)
    }
    fn render_from_node(&mut self, value: &protobuf::Node, depth: usize) -> Result<String, Error> {
        if depth > 8 || self.tables.len() > 8 {
            return Err(Error::Limit);
        }
        match node(value)? {
            NodeEnum::RangeVar(table) => {
                if self.tables.len() >= 8 {
                    return Err(Error::Limit);
                }
                if !table.catalogname.is_empty()
                    || table.schemaname.is_empty()
                    || table.alias.as_ref().is_some_and(|a| !a.colnames.is_empty())
                {
                    return Err(Error::UnsupportedSql);
                }
                let schema = Identifier::new(table.schemaname.clone())?;
                if schema.as_str() == "pg_catalog"
                    || schema.as_str() == "information_schema"
                    || schema.as_str().starts_with("pg_")
                {
                    return Err(Error::UnsupportedSql);
                }
                let name = Identifier::new(table.relname.clone())?;
                let alias = Identifier::new(
                    table
                        .alias
                        .as_ref()
                        .map_or_else(|| table.relname.clone(), |a| a.aliasname.clone()),
                )?;
                if self.tables.iter().any(|t| t.alias == alias) {
                    return Err(Error::UnsupportedSql);
                }
                let from = format!("ONLY {}.{} AS {}", schema.quoted(), name.quoted(), alias.quoted());
                self.tables.push(Table { schema, name, alias });
                Ok(from)
            }
            NodeEnum::JoinExpr(join) => {
                if join.is_natural
                    || !join.using_clause.is_empty()
                    || join.alias.is_some()
                    || join.join_using_alias.is_some()
                {
                    return Err(Error::UnsupportedSql);
                }
                let kind = match protobuf::JoinType::try_from(join.jointype) {
                    Ok(protobuf::JoinType::JoinInner) => "INNER",
                    Ok(protobuf::JoinType::JoinLeft) => "LEFT",
                    _ => return Err(Error::UnsupportedSql),
                };
                let scope_start = self.tables.len();
                let left = self.render_from_node(join.larg.as_deref().ok_or(Error::UnsupportedSql)?, depth + 1)?;
                let right = self.render_from_node(join.rarg.as_deref().ok_or(Error::UnsupportedSql)?, depth + 1)?;
                // Join predicates are rendered later after every column has been resolved.
                let predicate = expr(join.quals.as_deref().ok_or(Error::UnsupportedSql)?, 0)?;
                check_join_scope(&predicate, &self.tables[scope_start..])?;
                self.predicates.push(predicate);
                if kind == "LEFT" {
                    return Err(Error::UnsupportedSql);
                }
                Ok(format!("({left} {kind} JOIN {right} ON true)"))
            }
            _ => Err(Error::UnsupportedSql),
        }
    }
}
fn node(value: &protobuf::Node) -> Result<&NodeEnum, Error> {
    value.node.as_ref().ok_or(Error::UnsupportedSql)
}
fn string(value: &protobuf::Node) -> Result<String, Error> {
    match node(value)? {
        NodeEnum::String(value) => Ok(value.sval.clone()),
        _ => Err(Error::UnsupportedSql),
    }
}
fn column(value: &protobuf::Node) -> Result<Column, Error> {
    let NodeEnum::ColumnRef(value) = node(value)? else {
        return Err(Error::UnsupportedSql);
    };
    match value.fields.as_slice() {
        [name] => Ok(Column {
            qualifier: None,
            name: Identifier::new(string(name)?)?,
        }),
        [qualifier, name] => Ok(Column {
            qualifier: Some(Identifier::new(string(qualifier)?)?),
            name: Identifier::new(string(name)?)?,
        }),
        _ => Err(Error::UnsupportedSql),
    }
}
fn expr(value: &protobuf::Node, depth: usize) -> Result<Expr, Error> {
    if depth > 16 {
        return Err(Error::Limit);
    }
    match node(value)? {
        NodeEnum::ColumnRef(_) => Ok(Expr::Column(column(value)?)),
        NodeEnum::ParamRef(value) if (1..=32).contains(&value.number) => Ok(Expr::Parameter(value.number as usize)),
        NodeEnum::AConst(value) if !value.isnull => match value.val.as_ref() {
            Some(a_const::Val::Ival(value)) => Ok(Expr::Literal(value.ival.to_string())),
            Some(a_const::Val::Sval(value)) => Ok(Expr::Literal(format!("'{}'", value.sval.replace('\'', "''")))),
            Some(a_const::Val::Boolval(value)) => {
                Ok(Expr::Literal(if value.boolval { "true" } else { "false" }.into()))
            }
            _ => Err(Error::UnsupportedSql),
        },
        NodeEnum::AExpr(value) if value.kind == protobuf::AExprKind::AexprOp as i32 && value.name.len() == 1 => {
            let operator = string(&value.name[0])?;
            if !["=", "<>", "<", ">", "<=", ">="].contains(&operator.as_str()) {
                return Err(Error::UnsupportedSql);
            }
            Ok(Expr::Compare(
                Box::new(expr(value.lexpr.as_deref().ok_or(Error::UnsupportedSql)?, depth + 1)?),
                operator,
                Box::new(expr(value.rexpr.as_deref().ok_or(Error::UnsupportedSql)?, depth + 1)?),
            ))
        }
        NodeEnum::BoolExpr(value) => {
            let operator = match protobuf::BoolExprType::try_from(value.boolop) {
                Ok(protobuf::BoolExprType::AndExpr) => "AND",
                Ok(protobuf::BoolExprType::OrExpr) => "OR",
                _ => return Err(Error::UnsupportedSql),
            };
            if value.args.len() < 2 {
                return Err(Error::UnsupportedSql);
            }
            Ok(Expr::Boolean(
                value
                    .args
                    .iter()
                    .map(|v| expr(v, depth + 1))
                    .collect::<Result<_, _>>()?,
                operator,
            ))
        }
        _ => Err(Error::UnsupportedSql),
    }
}

fn check_join_scope(expr: &Expr, tables: &[Table]) -> Result<(), Error> {
    match expr {
        Expr::Column(column) => {
            if column
                .qualifier
                .as_ref()
                .is_some_and(|q| tables.iter().any(|t| t.alias == *q))
            {
                Ok(())
            } else {
                Err(Error::UnsupportedSql)
            }
        }
        Expr::Literal(_) | Expr::Parameter(_) => Ok(()),
        Expr::Compare(left, _, right) => {
            check_join_scope(left, tables)?;
            check_join_scope(right, tables)
        }
        Expr::Boolean(args, _) => {
            for expr in args {
                check_join_scope(expr, tables)?;
            }
            Ok(())
        }
    }
}

#[cfg(test)]
#[path = "parser_tests.rs"]
mod tests;
