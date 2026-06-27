use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::bundle::{
    Tier1DeclarativeBundle, Tier1DeclarativeEffect, Tier1DeclarativeMatch, Tier1DeclarativeRule,
    Tier1DeclarativeRuleId, Tier1DeclarativeSchemaError, Tier1ResourceKind,
};
use super::time_predicate::TimeOfDayWindow;

pub const TIER1_BUNDLE_EXTENSION: &str = "tier1.toml";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Tier1DeclarativeLoadError {
    #[error("read bundle dir {} failed: {message}", path.display())]
    ReadDir { path: PathBuf, message: String },
    #[error("read bundle file {} failed: {message}", path.display())]
    ReadFile { path: PathBuf, message: String },
    #[error("parse bundle file {} failed: {message}", path.display())]
    ParseToml { path: PathBuf, message: String },
    #[error("invalid bundle file {}: {error}", path.display())]
    Schema {
        path: PathBuf,
        #[source]
        error: Tier1DeclarativeSchemaError,
    },
}

#[derive(Debug, Deserialize)]
struct BundleFileToml {
    #[serde(default)]
    rule: Vec<RuleToml>,
}

#[derive(Debug, Deserialize)]
struct RuleToml {
    id: String,
    priority: u32,
    effect: String,
    #[serde(default)]
    matches: Vec<MatchToml>,
}

#[derive(Debug, Deserialize)]
struct MatchToml {
    kind: String,
    pattern: String,
    #[serde(default)]
    negate: bool,
}

impl Tier1DeclarativeBundle {
    pub fn load_from_dir(bundle_dir: impl AsRef<Path>) -> Result<Self, Tier1DeclarativeLoadError> {
        let bundle_dir = bundle_dir.as_ref();
        let mut rules = Vec::new();

        if bundle_dir.is_dir() {
            for entry in fs::read_dir(bundle_dir).map_err(|err| Tier1DeclarativeLoadError::ReadDir {
                path: bundle_dir.to_path_buf(),
                message: err.to_string(),
            })? {
                let entry = entry.map_err(|err| Tier1DeclarativeLoadError::ReadDir {
                    path: bundle_dir.to_path_buf(),
                    message: err.to_string(),
                })?;
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("toml") {
                    continue;
                }
                if !path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with(".tier1.toml"))
                {
                    continue;
                }
                rules.extend(parse_bundle_file(&path)?);
            }
        }

        Ok(Self::new(rules))
    }
}

fn parse_bundle_file(path: &Path) -> Result<Vec<Tier1DeclarativeRule>, Tier1DeclarativeLoadError> {
    let raw = fs::read_to_string(path).map_err(|err| Tier1DeclarativeLoadError::ReadFile {
        path: path.to_path_buf(),
        message: err.to_string(),
    })?;
    let parsed: BundleFileToml = toml::from_str(&raw).map_err(|err| Tier1DeclarativeLoadError::ParseToml {
        path: path.to_path_buf(),
        message: err.to_string(),
    })?;

    parsed.rule.into_iter().map(|rule| convert_rule(path, rule)).collect()
}

fn convert_rule(path: &Path, rule: RuleToml) -> Result<Tier1DeclarativeRule, Tier1DeclarativeLoadError> {
    if rule.id.trim().is_empty() {
        return Err(Tier1DeclarativeLoadError::Schema {
            path: path.to_path_buf(),
            error: Tier1DeclarativeSchemaError::EmptyRuleId,
        });
    }

    let matches = rule
        .matches
        .into_iter()
        .map(|item| convert_match(path, item))
        .collect::<Result<Vec<_>, _>>()?;

    let effect =
        Tier1DeclarativeEffect::parse(rule.effect.trim()).map_err(|error| Tier1DeclarativeLoadError::Schema {
            path: path.to_path_buf(),
            error,
        })?;

    Ok(Tier1DeclarativeRule {
        id: Tier1DeclarativeRuleId::new(rule.id),
        matches,
        effect,
        priority: rule.priority,
    })
}

fn convert_match(path: &Path, item: MatchToml) -> Result<Tier1DeclarativeMatch, Tier1DeclarativeLoadError> {
    if item.pattern.trim().is_empty() {
        return Err(Tier1DeclarativeLoadError::Schema {
            path: path.to_path_buf(),
            error: Tier1DeclarativeSchemaError::EmptyPattern,
        });
    }

    let kind = Tier1ResourceKind::parse(item.kind.trim()).map_err(|error| Tier1DeclarativeLoadError::Schema {
        path: path.to_path_buf(),
        error,
    })?;

    if kind == Tier1ResourceKind::TimeOfDay {
        TimeOfDayWindow::parse(item.pattern.trim()).map_err(|error| Tier1DeclarativeLoadError::Schema {
            path: path.to_path_buf(),
            error: Tier1DeclarativeSchemaError::InvalidTimeOfDayPattern(error.to_string()),
        })?;
    }

    Ok(Tier1DeclarativeMatch::new(kind, item.pattern, item.negate))
}

#[cfg(test)]
mod tests;
