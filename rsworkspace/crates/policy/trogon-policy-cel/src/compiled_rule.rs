use std::thread;

use cel_parser::{Expression, Parser};
use trogon_policy_eval::RuleName;

use crate::cel_limits::CelLimits;
use crate::compile_error::CompileError;
use crate::constants::{MAX_SOURCE_BYTES, PARSER_STACK_BYTES};
use crate::cost_model::static_cost;
use crate::lexical_guard::check_nesting;

/// One rule, parsed and checked once, ready to run against many requests.
///
/// A rule is a CEL predicate over the request facts. A request passes the rule
/// when the expression is the boolean `true`. Anything else refuses it, and the
/// rule has no way to say that a request is allowed beyond passing.
#[derive(Debug, Clone)]
pub struct CompiledRule {
    name: RuleName,
    expression: Expression,
    cost: u64,
}

impl CompiledRule {
    /// Parses and checks a rule, refusing it when it is empty, too long, too
    /// deep, not an expression, built from something the interpreter cannot
    /// run, or able to cost more than the limit.
    pub fn compile(name: RuleName, source: &str, limits: &CelLimits) -> Result<Self, CompileError> {
        if source.trim().is_empty() {
            return Err(CompileError::Empty);
        }
        if source.len() > MAX_SOURCE_BYTES {
            return Err(CompileError::SourceTooLong {
                actual: source.len(),
                limit: MAX_SOURCE_BYTES,
            });
        }
        check_nesting(source)?;
        let expression = parse(source)?;
        let cost = static_cost(&expression, limits)?;
        if cost > limits.max_cost() {
            return Err(CompileError::CostExceeded {
                cost,
                limit: limits.max_cost(),
            });
        }
        Ok(Self { name, expression, cost })
    }

    /// The name a refusal reports.
    pub fn name(&self) -> &RuleName {
        &self.name
    }

    /// The static upper bound on what the rule costs to run.
    pub fn cost(&self) -> u64 {
        self.cost
    }

    pub(crate) fn expression(&self) -> &Expression {
        &self.expression
    }
}

/// Parses on a thread of its own with a large stack.
///
/// The parser recurses through a dozen grammar rules per level of nesting and
/// its frames are large, so the stack of whichever thread happens to load the
/// bundle is not something to stake the process on. The thread lives for one
/// parse, and compiling is off the request path. A parser panic is caught by
/// the join and reported as a parse failure.
fn parse(source: &str) -> Result<Expression, CompileError> {
    let failure = |message: &str| CompileError::Parse {
        message: message.to_owned(),
    };
    thread::scope(|scope| {
        let parser = thread::Builder::new()
            .name("cel-rule-parser".to_owned())
            .stack_size(PARSER_STACK_BYTES)
            .spawn_scoped(scope, || {
                Parser::new().parse(source).map_err(|errors| CompileError::Parse {
                    message: errors.to_string(),
                })
            })
            .map_err(|_| failure("the parser thread could not be started"))?;
        parser.join().map_err(|_| failure("the parser failed on this source"))?
    })
}
