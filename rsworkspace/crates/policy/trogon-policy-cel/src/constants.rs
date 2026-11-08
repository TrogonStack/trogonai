//! Limits, weights, and names shared across this crate.

use std::time::Duration;

/// The static cost an expression may have unless the caller sets another limit.
pub const DEFAULT_MAX_COST: u64 = 5_000;

/// The wall-clock time one evaluation may take unless the caller sets another.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_millis(25);

/// The size of the facts one evaluation may read, in bytes of strings, keys,
/// and 8 bytes per scalar.
pub const DEFAULT_MAX_INPUT_BYTES: usize = 64 * 1024;

/// The bytes charged against the input limit for a value that holds no text.
pub const SCALAR_BYTES: usize = 8;

/// The longest list, and the largest map, a fact may hold.
pub const DEFAULT_MAX_COLLECTION_LEN: usize = 128;

/// The longest rule source accepted, in bytes.
pub const MAX_SOURCE_BYTES: usize = 8 * 1024;

/// The deepest expression nesting accepted. The parser and the interpreter both
/// recurse, so an unbounded depth is a stack overflow waiting for input.
pub const MAX_EXPRESSION_DEPTH: usize = 64;

/// The stack of the thread a rule is parsed on, in bytes. It is address space
/// the thread reserves and touches only as deep as the parse goes.
pub const PARSER_STACK_BYTES: usize = 64 * 1024 * 1024;

/// The deepest nesting of lists and maps accepted inside one fact.
pub const MAX_FACT_NESTING: usize = 16;

/// The longest fact variable name, in bytes.
pub const MAX_FACT_NAME_BYTES: usize = 64;

/// The most rules one rule set may hold.
pub const MAX_RULES_PER_SET: usize = 256;

/// The extension of a rule file in a bundle directory.
pub const RULE_FILE_EXTENSION: &str = "cel";

/// What visiting one literal, identifier, or selection costs.
pub const NODE_COST: u64 = 1;

/// What one function or operator call costs, before its operands.
pub const CALL_COST: u64 = 2;

/// What a call that scans a string costs, before its operands.
pub const STRING_SCAN_COST: u64 = 8;

/// What a regular expression match costs, before its operands.
pub const REGEX_COST: u64 = 64;

/// Calls that scan a string argument.
pub const STRING_SCAN_FUNCTIONS: &[&str] = &["contains", "startsWith", "endsWith", "@in"];

/// Calls that run a regular expression.
pub const REGEX_FUNCTIONS: &[&str] = &["matches"];

/// Words the expression language reserves, which cannot name a fact.
pub const RESERVED_WORDS: &[&str] = &[
    "as",
    "break",
    "const",
    "continue",
    "else",
    "false",
    "for",
    "function",
    "if",
    "import",
    "in",
    "let",
    "loop",
    "namespace",
    "null",
    "package",
    "return",
    "true",
    "var",
    "void",
    "while",
];
