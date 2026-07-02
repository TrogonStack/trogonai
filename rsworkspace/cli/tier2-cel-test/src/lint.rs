//! Lint pass for `.cel` bundles (WI-08).
//!
//! Mirrors the value of AGT's `agt lint-policy` (`agent_compliance/lint_policy.py`)
//! without any Rego/Cedar machinery: parse each `.cel` file in a bundle
//! directory and flag a small, honestly-scoped set of issues.
//!
//! Heuristics implemented, in order of confidence:
//!
//! 1. **Unbound variable reference (error).** `Tier2EvaluationContext`
//!    binds exactly five top-level CEL variables: `request`, `caller`,
//!    `agent`, `task`, `headers` (see
//!    `a2a_gateway::policy::tier2_cel::evaluator::bind_evaluation_context`).
//!    A rule referencing anything else evaluates against an undefined CEL
//!    variable and always fails closed (`Tier2EvalError::Execution`,
//!    denying every request through `RuleName::evaluation_error()`),
//!    functionally dead policy that silently masks its own rule name in
//!    the audit trail. Detected via `cel_interpreter::Program::references()`,
//!    which walks the parsed AST and returns root identifiers (e.g.
//!    `request` for `request.method`), so this catches every reference
//!    without re-implementing a CEL walker.
//! 2. **Duplicate rule body (warning).** Two or more `.cel` files whose
//!    trimmed source text is byte-identical. Same predicate enforced
//!    under two rule names; the second file changes nothing beyond
//!    which name appears in the audit log on a match.
//! 3. **Contradictory rule pair (warning).** Two `.cel` files where one's
//!    trimmed source is the exact syntactic negation of the other's
//!    (`!(<other>)`, `!<other>`, or the reverse). Because every Tier-2
//!    rule must independently evaluate `true` for a request to be
//!    allowed, a literal-negation pair means no request can ever satisfy
//!    both, so one of the two rules is unsatisfiable in combination with
//!    the other and should be removed or reworded. This is a syntactic
//!    check only (string match after stripping one layer of `!(...)` /
//!    `!`), not a semantic equivalence check.
//! 4. **Unreachable rule (warning).** [`Tier2CompiledBundle::snapshot`]
//!    evaluates rules in `RuleName`-sorted order and denies-and-stops on
//!    the first `false`; a rule whose source is the literal `false` can
//!    never be preceded by a request reaching a later rule in sort order,
//!    so anything after it (alphabetically) is dead policy. This is a
//!    conservative, source-literal check; it does not attempt general
//!    CEL satisfiability analysis, which is a much larger undertaking
//!    with no bound on false positives/negatives for a hand-written
//!    heuristic.
//!
//! What this pass deliberately does NOT attempt: general contradiction
//! detection between two different non-trivial expressions (e.g. `a > 5`
//! vs `a < 3` on the same field) requires an SMT-style solver over CEL
//! semantics, which is out of scope for a lint pass this size and would
//! trade a simple, auditable heuristic for a probabilistic one.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use cel_interpreter::Program;

const BOUND_VARIABLES: [&str; 5] = ["request", "caller", "agent", "task", "headers"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Error => "error",
            Self::Warning => "warning",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LintFinding {
    pub severity: Severity,
    pub file: PathBuf,
    pub message: String,
}

impl std::fmt::Display for LintFinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}: {}", self.file.display(), self.severity, self.message)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LintError {
    #[error("read bundle dir {}", .path.display())]
    ReadDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("read cel file {}", .path.display())]
    ReadFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug)]
pub struct LintReport {
    pub findings: Vec<LintFinding>,
}

impl LintReport {
    pub fn has_errors(&self) -> bool {
        self.findings.iter().any(|f| f.severity == Severity::Error)
    }

    pub fn errors(&self) -> impl Iterator<Item = &LintFinding> {
        self.findings.iter().filter(|f| f.severity == Severity::Error)
    }

    pub fn warnings(&self) -> impl Iterator<Item = &LintFinding> {
        self.findings.iter().filter(|f| f.severity == Severity::Warning)
    }
}

/// Lint every `*.cel` file directly inside `bundle_dir`. Files are visited
/// in sorted-path order, matching [`a2a_gateway::policy::Tier2CompiledBundle`]'s
/// own load order, so the unreachable-rule heuristic (which depends on
/// evaluation order) sees the same sequence the evaluator does.
pub fn lint_bundle(bundle_dir: &Path) -> Result<LintReport, LintError> {
    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in fs::read_dir(bundle_dir).map_err(|source| LintError::ReadDir {
        path: bundle_dir.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| LintError::ReadDir {
            path: bundle_dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("cel") {
            paths.push(path);
        }
    }
    paths.sort();

    let mut findings = Vec::new();
    let mut sources: Vec<(PathBuf, String)> = Vec::new();
    for path in &paths {
        let source = fs::read_to_string(path).map_err(|source| LintError::ReadFile {
            path: path.clone(),
            source,
        })?;
        sources.push((path.clone(), source));
    }

    for (path, source) in &sources {
        lint_unbound_variables(path, source, &mut findings);
    }

    lint_duplicate_rules(&sources, &mut findings);
    lint_contradictory_rules(&sources, &mut findings);
    lint_unreachable_after_always_false(&sources, &mut findings);

    Ok(LintReport { findings })
}

fn lint_unbound_variables(path: &Path, source: &str, findings: &mut Vec<LintFinding>) {
    let program = match Program::compile(source) {
        Ok(program) => program,
        Err(err) => {
            findings.push(LintFinding {
                severity: Severity::Error,
                file: path.to_path_buf(),
                message: format!("failed to parse CEL source: {err}"),
            });
            return;
        }
    };
    let references = program.references();
    let mut unbound: Vec<&str> = references
        .variables()
        .into_iter()
        .filter(|name| !BOUND_VARIABLES.contains(name))
        .collect();
    unbound.sort_unstable();
    for name in unbound {
        findings.push(LintFinding {
            severity: Severity::Error,
            file: path.to_path_buf(),
            message: format!(
                "references unbound variable '{name}' (only request, caller, agent, task, headers are bound)"
            ),
        });
    }
}

fn lint_duplicate_rules(sources: &[(PathBuf, String)], findings: &mut Vec<LintFinding>) {
    let mut by_body: BTreeMap<&str, Vec<&Path>> = BTreeMap::new();
    for (path, source) in sources {
        by_body.entry(source.trim()).or_default().push(path);
    }
    for (_, paths) in by_body {
        if paths.len() < 2 {
            continue;
        }
        for path in &paths[1..] {
            findings.push(LintFinding {
                severity: Severity::Warning,
                file: (*path).to_path_buf(),
                message: format!(
                    "duplicate rule body also found in {}",
                    paths[0].file_name().map(|n| n.to_string_lossy()).unwrap_or_default()
                ),
            });
        }
    }
}

/// Strip one layer of `!(...)` or `!` negation from a trimmed CEL source,
/// returning the inner text if `source` is syntactically negated.
fn strip_negation(source: &str) -> Option<&str> {
    let rest = source.strip_prefix('!')?;
    if let Some(inner) = rest.strip_prefix('(') {
        inner.strip_suffix(')')
    } else {
        Some(rest)
    }
}

fn lint_contradictory_rules(sources: &[(PathBuf, String)], findings: &mut Vec<LintFinding>) {
    for i in 0..sources.len() {
        for j in (i + 1)..sources.len() {
            let (path_a, source_a) = &sources[i];
            let (path_b, source_b) = &sources[j];
            let a = source_a.trim();
            let b = source_b.trim();
            let negated_match =
                strip_negation(a).is_some_and(|inner| inner == b) || strip_negation(b).is_some_and(|inner| inner == a);
            if negated_match {
                findings.push(LintFinding {
                    severity: Severity::Warning,
                    file: path_b.to_path_buf(),
                    message: format!(
                        "contradicts rule '{}': one is the exact negation of the other, so no request can satisfy both",
                        path_a.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()
                    ),
                });
            }
        }
    }
}

fn lint_unreachable_after_always_false(sources: &[(PathBuf, String)], findings: &mut Vec<LintFinding>) {
    let Some(always_false_index) = sources.iter().position(|(_, source)| source.trim() == "false") else {
        return;
    };
    for (path, _) in &sources[always_false_index + 1..] {
        findings.push(LintFinding {
            severity: Severity::Warning,
            file: path.to_path_buf(),
            message: format!(
                "unreachable: sorts after unconditional deny rule '{}', which always denies first",
                sources[always_false_index]
                    .0
                    .file_name()
                    .map(|n| n.to_string_lossy())
                    .unwrap_or_default()
            ),
        });
    }
}

#[cfg(test)]
mod tests;
