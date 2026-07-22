use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::bundle::{
    Tier1DeclarativeBundle, Tier1DeclarativeEffect, Tier1DeclarativeMatch, Tier1DeclarativeRule,
    Tier1DeclarativeRuleId, Tier1DeclarativeSchemaError, Tier1ResourceKind,
};

pub const TIER1_BUNDLE_EXTENSION: &str = "tier1.toml";

#[derive(Debug, thiserror::Error)]
pub enum Tier1DeclarativeLoadError {
    /// An existing path was configured but it is not a directory. We
    /// surface a typed error here so the runtime fails fast at boot
    /// instead of falling through to default-allow with no rules loaded.
    #[error("bundle dir {} exists but is not a directory", path.display())]
    NotDirectory { path: PathBuf },
    #[error("read bundle dir {} failed", path.display())]
    ReadDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("read bundle file {} failed", path.display())]
    ReadFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("parse bundle file {} failed", path.display())]
    ParseToml {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("invalid bundle file {}", path.display())]
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
    /// Load all `*.tier1.toml` bundles from the configured directory.
    ///
    /// A missing directory yields an empty bundle so operators can stage
    /// the path before populating it. An existing path that is *not* a
    /// directory is a misconfiguration and fails fast — without that
    /// check the runtime would silently default-allow every request.
    pub fn load_from_dir(bundle_dir: impl AsRef<Path>) -> Result<Self, Tier1DeclarativeLoadError> {
        let bundle_dir = bundle_dir.as_ref();
        let mut rules = Vec::new();

        if !bundle_dir.exists() {
            return Ok(Self::new(rules));
        }
        if !bundle_dir.is_dir() {
            return Err(Tier1DeclarativeLoadError::NotDirectory {
                path: bundle_dir.to_path_buf(),
            });
        }

        // Collect first, sort by path, THEN parse. `read_dir` order is
        // filesystem-dependent — without a deterministic file ordering,
        // equal-priority allow/deny rules from different files could
        // evaluate non-deterministically across hosts or restarts.
        let mut bundle_paths: Vec<PathBuf> = Vec::new();
        for entry in fs::read_dir(bundle_dir).map_err(|source| Tier1DeclarativeLoadError::ReadDir {
            path: bundle_dir.to_path_buf(),
            source,
        })? {
            let entry = entry.map_err(|source| Tier1DeclarativeLoadError::ReadDir {
                path: bundle_dir.to_path_buf(),
                source,
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
            bundle_paths.push(path);
        }
        bundle_paths.sort();
        for path in bundle_paths {
            rules.extend(parse_bundle_file(&path)?);
        }

        Ok(Self::new(rules))
    }
}

fn parse_bundle_file(path: &Path) -> Result<Vec<Tier1DeclarativeRule>, Tier1DeclarativeLoadError> {
    let raw = fs::read_to_string(path).map_err(|source| Tier1DeclarativeLoadError::ReadFile {
        path: path.to_path_buf(),
        source,
    })?;
    let parsed: BundleFileToml = toml::from_str(&raw).map_err(|source| Tier1DeclarativeLoadError::ParseToml {
        path: path.to_path_buf(),
        source,
    })?;

    parsed.rule.into_iter().map(|rule| convert_rule(path, rule)).collect()
}

fn convert_rule(path: &Path, rule: RuleToml) -> Result<Tier1DeclarativeRule, Tier1DeclarativeLoadError> {
    let id = Tier1DeclarativeRuleId::new(rule.id.clone()).map_err(|error| Tier1DeclarativeLoadError::Schema {
        path: path.to_path_buf(),
        error,
    })?;

    let matches = rule
        .matches
        .into_iter()
        .map(|item| convert_match(path, item))
        .collect::<Result<Vec<_>, _>>()?;

    // Reject rules with no `[[rule.matches]]` block at load time so a
    // typo or partial config can't apply a high-priority deny or allow
    // to every request — `iter().all()` returns true for an empty
    // iterator, which would make a match-less rule fire on every
    // ingress request without any matcher.
    if matches.is_empty() {
        return Err(Tier1DeclarativeLoadError::Schema {
            path: path.to_path_buf(),
            error: Tier1DeclarativeSchemaError::NoMatches(rule.id),
        });
    }

    let effect =
        Tier1DeclarativeEffect::parse(rule.effect.trim()).map_err(|error| Tier1DeclarativeLoadError::Schema {
            path: path.to_path_buf(),
            error,
        })?;

    Ok(Tier1DeclarativeRule {
        id,
        matches,
        effect,
        priority: rule.priority,
    })
}

fn convert_match(path: &Path, item: MatchToml) -> Result<Tier1DeclarativeMatch, Tier1DeclarativeLoadError> {
    let kind = Tier1ResourceKind::parse(item.kind.trim()).map_err(|error| Tier1DeclarativeLoadError::Schema {
        path: path.to_path_buf(),
        error,
    })?;

    Tier1DeclarativeMatch::new(kind, item.pattern, item.negate).map_err(|error| Tier1DeclarativeLoadError::Schema {
        path: path.to_path_buf(),
        error,
    })
}

#[cfg(test)]
mod tests;
