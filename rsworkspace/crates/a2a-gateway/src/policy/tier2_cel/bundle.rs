use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use cel_interpreter::Program;

use super::bundle_load_error::Tier2BundleLoadError;
use super::compiler::{self, CelCompileError};
use crate::policy::tier2::rule_name::RuleName;
use crate::policy::tier2_dynamic::{Tier2DynamicCondition, load_dynamic_condition_sidecar};

#[derive(Clone, Debug)]
pub struct CelProgramHandle(Arc<Program>);

impl CelProgramHandle {
    pub fn new(program: Arc<Program>) -> Self {
        Self(program)
    }

    pub fn program(&self) -> &Program {
        &self.0
    }
}

#[derive(Debug)]
struct CachedRule {
    path: PathBuf,
    mtime: SystemTime,
    program: CelProgramHandle,
    dynamic_condition: Option<Tier2DynamicCondition>,
}

#[derive(Debug)]
pub struct Tier2CompiledBundle {
    tier2_dir: PathBuf,
    rules: BTreeMap<RuleName, CachedRule>,
}

impl Tier2CompiledBundle {
    /// Load all `*.cel` rules from the configured directory.
    ///
    /// A missing directory yields an empty bundle so operators can stage
    /// the path before populating it. An existing path that is NOT a
    /// directory is a misconfiguration and fails fast — without this
    /// check the runtime would silently default-allow with no rules
    /// loaded.
    pub fn load_from_dir(tier2_dir: impl Into<PathBuf>) -> Result<Self, Tier2BundleLoadError> {
        let tier2_dir = tier2_dir.into();
        let mut rules = BTreeMap::new();
        if !tier2_dir.exists() {
            return Ok(Self { tier2_dir, rules });
        }
        if !tier2_dir.is_dir() {
            // No dedicated NotDirectory variant on CelCompileError —
            // surface the misconfiguration through Read with an
            // InvalidInput io error so the audit chain still carries
            // the typed source.
            return Err(CelCompileError::Read {
                path: tier2_dir.clone(),
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "tier-2 bundle path exists but is not a directory",
                ),
            }
            .into());
        }
        // Collect first, sort by path, THEN parse. `read_dir` order is
        // filesystem-dependent — sorting paths up front keeps rule load
        // order reproducible across hosts and restarts.
        let mut cel_paths: Vec<PathBuf> = Vec::new();
        for entry in fs::read_dir(&tier2_dir).map_err(|source| CelCompileError::Read {
            path: tier2_dir.clone(),
            source,
        })? {
            let entry = entry.map_err(|source| CelCompileError::Read {
                path: tier2_dir.clone(),
                source,
            })?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("cel") {
                continue;
            }
            cel_paths.push(path);
        }
        cel_paths.sort();
        for path in cel_paths {
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            // Skip files that turn into an empty stem (e.g. `.cel` with
            // no name) so the unchecked constructor only ever sees the
            // non-empty input it requires.
            if stem.trim().is_empty() {
                continue;
            }
            let rule = RuleName::new_unchecked(stem);
            let (program, mtime) = compiler::compile_cel_file(&path)?;
            let dynamic_condition = load_dynamic_condition_sidecar(&path)?;
            rules.insert(
                rule,
                CachedRule {
                    path,
                    mtime,
                    program,
                    dynamic_condition,
                },
            );
        }
        Ok(Self { tier2_dir, rules })
    }

    /// Refresh cached rules in-place. Picks up content changes via mtime,
    /// drops cached entries whose source file was removed/renamed
    /// (otherwise the policy would keep enforcing a rule the operator
    /// deleted), and adds entries for new `.cel` files in the directory.
    ///
    /// A `.dynamic.toml` sidecar is reloaded whenever its `.cel` sibling's
    /// mtime changes -- sidecar-only edits (no `.cel` content change) are
    /// picked up on the next full bundle reload rather than tracked with
    /// a second mtime, keeping the staleness check to one filesystem stat
    /// per rule.
    pub fn refresh_if_stale(&mut self) -> Result<(), Tier2BundleLoadError> {
        let mut removed: Vec<RuleName> = Vec::new();
        for (name, cached) in self.rules.iter_mut() {
            match fs::metadata(&cached.path) {
                Ok(meta) => {
                    let current_mtime = meta.modified().map_err(|source| CelCompileError::Metadata {
                        path: cached.path.clone(),
                        source,
                    })?;
                    if current_mtime != cached.mtime {
                        let (program, mtime) = compiler::compile_cel_file(&cached.path)?;
                        cached.dynamic_condition = load_dynamic_condition_sidecar(&cached.path)?;
                        cached.program = program;
                        cached.mtime = mtime;
                    }
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    removed.push(name.clone());
                }
                Err(source) => {
                    return Err(CelCompileError::Metadata {
                        path: cached.path.clone(),
                        source,
                    }
                    .into());
                }
            }
        }
        for name in removed {
            self.rules.remove(&name);
        }
        self.reload_new_files()?;
        Ok(())
    }

    pub fn rules(&self) -> impl Iterator<Item = (&RuleName, &CelProgramHandle)> {
        self.rules.iter().map(|(name, cached)| (name, &cached.program))
    }

    /// Snapshot the current rule set as cheap-to-clone Arc handles so an
    /// evaluator can iterate without holding the bundle lock across the
    /// CEL execution path. Each entry carries its optional dynamic
    /// condition alongside the compiled program so the evaluator can
    /// apply AND semantics without a second lookup.
    pub fn snapshot(&self) -> Vec<(RuleName, CelProgramHandle, Option<Tier2DynamicCondition>)> {
        self.rules
            .iter()
            .map(|(name, cached)| (name.clone(), cached.program.clone(), cached.dynamic_condition.clone()))
            .collect()
    }

    pub fn tier2_dir(&self) -> &Path {
        &self.tier2_dir
    }

    fn reload_new_files(&mut self) -> Result<(), Tier2BundleLoadError> {
        if !self.tier2_dir.is_dir() {
            return Ok(());
        }
        let mut cel_paths: Vec<PathBuf> = Vec::new();
        for entry in fs::read_dir(&self.tier2_dir).map_err(|source| CelCompileError::Read {
            path: self.tier2_dir.clone(),
            source,
        })? {
            let entry = entry.map_err(|source| CelCompileError::Read {
                path: self.tier2_dir.clone(),
                source,
            })?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("cel") {
                continue;
            }
            cel_paths.push(path);
        }
        cel_paths.sort();
        for path in cel_paths {
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if stem.trim().is_empty() {
                continue;
            }
            let rule = RuleName::new_unchecked(stem);
            if self.rules.contains_key(&rule) {
                continue;
            }
            let (program, mtime) = compiler::compile_cel_file(&path)?;
            let dynamic_condition = load_dynamic_condition_sidecar(&path)?;
            self.rules.insert(
                rule,
                CachedRule {
                    path,
                    mtime,
                    program,
                    dynamic_condition,
                },
            );
        }
        Ok(())
    }
}
