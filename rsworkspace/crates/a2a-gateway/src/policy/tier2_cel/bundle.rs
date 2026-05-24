use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use cel_interpreter::Program;

use crate::policy::tier2::rule_name::RuleName;

use super::compiler::{self, CelCompileError};

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
}

#[derive(Debug)]
pub struct Tier2CompiledBundle {
    tier2_dir: PathBuf,
    rules: BTreeMap<RuleName, CachedRule>,
}

impl Tier2CompiledBundle {
    pub fn load_from_dir(tier2_dir: impl Into<PathBuf>) -> Result<Self, CelCompileError> {
        let tier2_dir = tier2_dir.into();
        let mut rules = BTreeMap::new();
        if tier2_dir.is_dir() {
            for entry in fs::read_dir(&tier2_dir).map_err(|err| {
                CelCompileError::for_path(&tier2_dir, format!("read_dir failed: {err}"))
            })? {
                let entry = entry.map_err(|err| {
                    CelCompileError::for_path(&tier2_dir, format!("read_dir entry failed: {err}"))
                })?;
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("cel") {
                    continue;
                }
                let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                let (program, mtime) = compiler::compile_cel_file(&path)?;
                rules.insert(
                    RuleName::new(stem),
                    CachedRule {
                        path,
                        mtime,
                        program,
                    },
                );
            }
        }
        Ok(Self { tier2_dir, rules })
    }

    pub fn refresh_if_stale(&mut self) -> Result<(), CelCompileError> {
        for cached in self.rules.values_mut() {
            let current_mtime = fs::metadata(&cached.path)
                .and_then(|meta| meta.modified())
                .map_err(|err| {
                    CelCompileError::for_path(&cached.path, format!("metadata failed: {err}"))
                })?;
            if current_mtime != cached.mtime {
                let (program, mtime) = compiler::compile_cel_file(&cached.path)?;
                cached.program = program;
                cached.mtime = mtime;
            }
        }
        self.reload_new_files()?;
        Ok(())
    }

    pub fn rules(&self) -> impl Iterator<Item = (&RuleName, &CelProgramHandle)> {
        self.rules
            .iter()
            .map(|(name, cached)| (name, &cached.program))
    }

    pub fn tier2_dir(&self) -> &Path {
        &self.tier2_dir
    }

    fn reload_new_files(&mut self) -> Result<(), CelCompileError> {
        if !self.tier2_dir.is_dir() {
            return Ok(());
        }
        for entry in fs::read_dir(&self.tier2_dir).map_err(|err| {
            CelCompileError::for_path(&self.tier2_dir, format!("read_dir failed: {err}"))
        })? {
            let entry = entry.map_err(|err| {
                CelCompileError::for_path(&self.tier2_dir, format!("read_dir entry failed: {err}"))
            })?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("cel") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let rule = RuleName::new(stem);
            if self.rules.contains_key(&rule) {
                continue;
            }
            let (program, mtime) = compiler::compile_cel_file(&path)?;
            self.rules.insert(
                rule,
                CachedRule {
                    path,
                    mtime,
                    program,
                },
            );
        }
        Ok(())
    }
}
