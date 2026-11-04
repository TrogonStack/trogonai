use cel_parser::Expression;
use cel_parser::ast::operators::{ADD, CONDITIONAL};
use cel_parser::ast::{EntryExpr, Expr};

use crate::cel_limits::CelLimits;
use crate::compile_error::CompileError;
use crate::constants::{
    CALL_COST, MAX_EXPRESSION_DEPTH, NODE_COST, REGEX_COST, REGEX_FUNCTIONS, STRING_SCAN_COST, STRING_SCAN_FUNCTIONS,
};

/// Walks a parsed rule, refuses what the interpreter cannot run, and returns
/// an upper bound on what running it costs.
///
/// The bound is static: it assumes every list and map the rule can reach is as
/// long as the limits allow, every branch is taken, and every loop runs to its
/// end. A rule whose bound is above the cost limit does not load, so no
/// request ever pays for it. The unit is a weight, not a measure of time: it
/// ranks work by the shape of the expression and the interpreter has no hook
/// to count anything at run time.
pub(crate) fn static_cost(expression: &Expression, limits: &CelLimits) -> Result<u64, CompileError> {
    let mut survey = Survey::default();
    survey.visit(expression, 1)?;
    let length_bound = survey.length_bound(limits.max_collection_len());
    cost_of(expression, length_bound)
}

/// What the first pass learns about the sizes a rule can build.
#[derive(Default)]
struct Survey {
    longest_literal: usize,
    concatenations: usize,
}

impl Survey {
    fn visit(&mut self, expression: &Expression, depth: usize) -> Result<(), CompileError> {
        if depth > MAX_EXPRESSION_DEPTH {
            return Err(CompileError::TooDeep {
                limit: MAX_EXPRESSION_DEPTH,
            });
        }
        match &expression.expr {
            Expr::List(list) => self.longest_literal = self.longest_literal.max(list.elements.len()),
            Expr::Map(map) => self.longest_literal = self.longest_literal.max(map.entries.len()),
            Expr::Call(call) if call.func_name == ADD => self.concatenations += 1,
            _ => {}
        }
        for child in children(expression)? {
            self.visit(child, depth + 1)?;
        }
        Ok(())
    }

    /// The longest list or map any part of the rule can hold: a fact at the
    /// limit, or a literal, and each concatenation can double it.
    fn length_bound(&self, max_collection_len: usize) -> u64 {
        let longest = u64::try_from(self.longest_literal.max(max_collection_len)).unwrap_or(u64::MAX);
        let growth = u64::try_from(self.concatenations).unwrap_or(u64::MAX).saturating_add(1);
        longest.saturating_mul(growth)
    }
}

/// Prices a rule the survey already accepted, so its depth is within the limit
/// and it holds no construct the interpreter cannot run.
fn cost_of(expression: &Expression, length_bound: u64) -> Result<u64, CompileError> {
    let below = |child: &Expression| cost_of(child, length_bound);
    match &expression.expr {
        Expr::Literal(_) | Expr::Ident(_) => Ok(NODE_COST),
        Expr::Select(select) => Ok(NODE_COST.saturating_add(below(&select.operand)?)),
        // `children` refuses a struct or an unspecified expression, so those
        // never reach a price.
        Expr::List(_) | Expr::Map(_) | Expr::Struct(_) | Expr::Unspecified => {
            let mut total = NODE_COST;
            for child in children(expression)? {
                total = total.saturating_add(below(child)?);
            }
            Ok(total)
        }
        Expr::Call(call) => {
            if call.func_name == CONDITIONAL && call.args.len() == 3 {
                let taken = below(&call.args[1])?.max(below(&call.args[2])?);
                return Ok(CALL_COST.saturating_add(below(&call.args[0])?).saturating_add(taken));
            }
            let mut total = call_weight(&call.func_name);
            for child in children(expression)? {
                total = total.saturating_add(below(child)?);
            }
            Ok(total)
        }
        Expr::Comprehension(comprehension) => {
            let per_iteration = below(&comprehension.loop_cond)?.saturating_add(below(&comprehension.loop_step)?);
            Ok(NODE_COST
                .saturating_add(below(&comprehension.iter_range)?)
                .saturating_add(below(&comprehension.accu_init)?)
                .saturating_add(per_iteration.saturating_mul(length_bound))
                .saturating_add(below(&comprehension.result)?))
        }
    }
}

fn call_weight(function: &str) -> u64 {
    if REGEX_FUNCTIONS.contains(&function) {
        REGEX_COST
    } else if STRING_SCAN_FUNCTIONS.contains(&function) {
        STRING_SCAN_COST
    } else {
        CALL_COST
    }
}

fn unsupported(expr: &Expr) -> CompileError {
    let construct = match expr {
        Expr::Struct(_) => "a struct literal",
        _ => "an unspecified expression",
    };
    CompileError::Unsupported { construct }
}

/// The direct sub-expressions of an expression, or the reason the interpreter
/// cannot evaluate it.
fn children(expression: &Expression) -> Result<Vec<&Expression>, CompileError> {
    match &expression.expr {
        Expr::Literal(_) | Expr::Ident(_) => Ok(Vec::new()),
        Expr::Select(select) => Ok(vec![&select.operand]),
        Expr::List(list) => Ok(list.elements.iter().collect()),
        Expr::Call(call) => Ok(call
            .target
            .iter()
            .map(|target| &**target)
            .chain(call.args.iter())
            .collect()),
        Expr::Comprehension(comprehension) => Ok(vec![
            &comprehension.iter_range,
            &comprehension.accu_init,
            &comprehension.loop_cond,
            &comprehension.loop_step,
            &comprehension.result,
        ]),
        Expr::Map(map) => {
            let mut parts = Vec::with_capacity(map.entries.len() * 2);
            for entry in &map.entries {
                match &entry.expr {
                    EntryExpr::MapEntry(pair) if !pair.optional => {
                        parts.push(&pair.key);
                        parts.push(&pair.value);
                    }
                    EntryExpr::MapEntry(_) => {
                        return Err(CompileError::Unsupported {
                            construct: "an optional map entry",
                        });
                    }
                    EntryExpr::StructField(_) => {
                        return Err(CompileError::Unsupported {
                            construct: "a struct field",
                        });
                    }
                }
            }
            Ok(parts)
        }
        Expr::Struct(_) | Expr::Unspecified => Err(unsupported(&expression.expr)),
    }
}

#[cfg(test)]
mod tests;
