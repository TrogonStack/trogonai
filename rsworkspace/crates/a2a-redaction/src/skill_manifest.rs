use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::Deserialize;

use crate::a2a_method::A2aMethod;
use crate::skill_id::SkillId;
use crate::wasm_bundle_path::WasmBundlePath;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct JsonPathExpr(String);

impl JsonPathExpr {
    pub fn new(expr: impl Into<String>) -> Self {
        Self(expr.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for JsonPathExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillManifestVersion(String);

impl SkillManifestVersion {
    pub fn new(version: impl Into<String>) -> Result<Self, SkillManifestError> {
        let version = version.into();
        if is_semver_shaped(&version) {
            Ok(Self(version))
        } else {
            Err(SkillManifestError::InvalidVersion { version })
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillCategory {
    Pii,
    Credentials,
    InternalRoute,
    RateLimit,
    Custom(String),
}

impl SkillCategory {
    fn priority(&self) -> u8 {
        match self {
            Self::InternalRoute => 0,
            Self::Credentials => 1,
            Self::Pii => 2,
            Self::RateLimit => 3,
            Self::Custom(_) => 4,
        }
    }

    fn parse(value: &str) -> Result<Self, SkillManifestError> {
        match value {
            "Pii" => Ok(Self::Pii),
            "Credentials" => Ok(Self::Credentials),
            "InternalRoute" => Ok(Self::InternalRoute),
            "RateLimit" => Ok(Self::RateLimit),
            other if other.starts_with("Custom:") => Ok(Self::Custom(other["Custom:".len()..].to_string())),
            other => Err(SkillManifestError::InvalidCategory {
                category: other.to_string(),
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillMethodMatcher {
    Any,
    OneOf(Vec<A2aMethod>),
}

impl SkillMethodMatcher {
    pub fn matches(&self, method: &A2aMethod) -> bool {
        match self {
            Self::Any => true,
            Self::OneOf(methods) => methods.contains(method),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillManifest {
    skill_id: SkillId,
    wasm_path: WasmBundlePath,
    applies_to_method: SkillMethodMatcher,
    applies_to_paths: Vec<JsonPathExpr>,
    category: SkillCategory,
    version: SkillManifestVersion,
}

impl SkillManifest {
    pub fn skill_id(&self) -> &SkillId {
        &self.skill_id
    }

    pub fn wasm_path(&self) -> &WasmBundlePath {
        &self.wasm_path
    }

    pub fn applies_to_method(&self) -> &SkillMethodMatcher {
        &self.applies_to_method
    }

    pub fn applies_to_paths(&self) -> &[JsonPathExpr] {
        &self.applies_to_paths
    }

    pub fn category(&self) -> &SkillCategory {
        &self.category
    }

    pub fn version(&self) -> &SkillManifestVersion {
        &self.version
    }

    pub fn resolve_wasm_path(&self, bundle_dir: &Path) -> PathBuf {
        bundle_dir.join(self.wasm_path.as_path())
    }

    fn matches_paths(&self, payload_paths: &[JsonPathExpr]) -> bool {
        if self.applies_to_paths.is_empty() {
            return false;
        }
        let payload: HashSet<&str> = payload_paths.iter().map(JsonPathExpr::as_str).collect();
        self.applies_to_paths
            .iter()
            .any(|path| payload.contains(path.as_str()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillManifestRegistry {
    manifests: HashMap<SkillId, SkillManifest>,
    sources: HashMap<SkillId, PathBuf>,
}

impl SkillManifestRegistry {
    pub fn new() -> Self {
        Self {
            manifests: HashMap::new(),
            sources: HashMap::new(),
        }
    }

    pub fn load_from_dir(bundle_dir: &Path) -> Result<Self, SkillManifestError> {
        let mut registry = Self::new();
        let entries = std::fs::read_dir(bundle_dir).map_err(|source| SkillManifestError::Io {
            path: bundle_dir.to_path_buf(),
            source,
        })?;

        for entry in entries {
            let entry = entry.map_err(|source| SkillManifestError::Io {
                path: bundle_dir.to_path_buf(),
                source,
            })?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("toml") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if !stem.ends_with(".skill") {
                continue;
            }
            let manifest = parse_manifest_file(&path)?;
            if let Some(first) = registry.sources.get(manifest.skill_id()) {
                return Err(SkillManifestError::DuplicateSkillId {
                    skill_id: manifest.skill_id().clone(),
                    first: first.clone(),
                    second: path,
                });
            }
            registry
                .sources
                .insert(manifest.skill_id().clone(), path.clone());
            registry
                .manifests
                .insert(manifest.skill_id().clone(), manifest);
        }

        Ok(registry)
    }

    pub fn insert(&mut self, manifest: SkillManifest) -> Result<(), SkillManifestError> {
        if self.manifests.contains_key(manifest.skill_id()) {
            return Err(SkillManifestError::DuplicateSkillId {
                skill_id: manifest.skill_id().clone(),
                first: self
                    .sources
                    .get(manifest.skill_id())
                    .cloned()
                    .unwrap_or_else(|| manifest.wasm_path().as_path().to_path_buf()),
                second: manifest.wasm_path().as_path().to_path_buf(),
            });
        }
        self.manifests.insert(manifest.skill_id().clone(), manifest);
        Ok(())
    }

    pub fn lookup(&self, skill_id: &SkillId) -> Option<&SkillManifest> {
        self.manifests.get(skill_id)
    }

    pub fn skills_for_method(&self, method: &A2aMethod) -> Vec<&SkillManifest> {
        let mut matches: Vec<&SkillManifest> = self
            .manifests
            .values()
            .filter(|manifest| manifest.applies_to_method().matches(method))
            .collect();
        matches.sort_by(|left, right| {
            left.category()
                .priority()
                .cmp(&right.category().priority())
                .then_with(|| left.skill_id().as_str().cmp(right.skill_id().as_str()))
        });
        matches
    }

    pub fn manifests(&self) -> impl Iterator<Item = &SkillManifest> {
        self.manifests.values()
    }
}

impl Default for SkillManifestRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillSelectionPlan {
    manifests: Vec<SkillManifest>,
}

impl SkillSelectionPlan {
    pub fn plan(
        registry: &SkillManifestRegistry,
        method: &A2aMethod,
        payload_paths: &[JsonPathExpr],
    ) -> Self {
        let mut manifests: Vec<SkillManifest> = registry
            .skills_for_method(method)
            .into_iter()
            .filter(|manifest| manifest.matches_paths(payload_paths))
            .cloned()
            .collect();
        manifests.sort_by(|left, right| {
            left.category()
                .priority()
                .cmp(&right.category().priority())
                .then_with(|| left.skill_id().as_str().cmp(right.skill_id().as_str()))
        });
        Self { manifests }
    }

    pub fn manifests(&self) -> &[SkillManifest] {
        &self.manifests
    }

    pub fn is_empty(&self) -> bool {
        self.manifests.is_empty()
    }
}

#[derive(Debug)]
pub enum SkillManifestError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
    MissingField {
        path: PathBuf,
        field: &'static str,
    },
    InvalidVersion {
        version: String,
    },
    DuplicateSkillId {
        skill_id: SkillId,
        first: PathBuf,
        second: PathBuf,
    },
    FilenameMismatch {
        path: PathBuf,
        expected: SkillId,
        found: SkillId,
    },
    InvalidMethod {
        path: PathBuf,
        method: String,
    },
    InvalidCategory {
        category: String,
    },
    EmptyPaths {
        path: PathBuf,
    },
}

impl fmt::Display for SkillManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(f, "read skill manifest dir {}: {source}", path.display())
            }
            Self::Parse { path, source } => {
                write!(f, "parse skill manifest {}: {source}", path.display())
            }
            Self::MissingField { path, field } => {
                write!(f, "skill manifest {} missing field `{field}`", path.display())
            }
            Self::InvalidVersion { version } => {
                write!(f, "invalid skill manifest version `{version}` (expected semver shape)")
            }
            Self::DuplicateSkillId {
                skill_id,
                first,
                second,
            } => write!(
                f,
                "duplicate skill_id `{skill_id}` between {} and {}",
                first.display(),
                second.display()
            ),
            Self::FilenameMismatch { path, expected, found } => write!(
                f,
                "skill manifest filename mismatch at {}: expected `{expected}`, found `{found}`",
                path.display()
            ),
            Self::InvalidMethod { path, method } => {
                write!(
                    f,
                    "skill manifest {} references unknown method `{method}`",
                    path.display()
                )
            }
            Self::InvalidCategory { category } => {
                write!(f, "invalid skill category `{category}`")
            }
            Self::EmptyPaths { path } => {
                write!(f, "skill manifest {} requires at least one applies_to_paths entry", path.display())
            }
        }
    }
}

impl std::error::Error for SkillManifestError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Parse { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[derive(Debug, Deserialize)]
struct RawSkillManifest {
    skill_id: Option<String>,
    wasm_path: Option<String>,
    applies_to_method: Option<RawMethodMatcher>,
    applies_to_paths: Option<Vec<String>>,
    category: Option<RawCategory>,
    version: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(non_snake_case)]
struct CustomCategoryTable {
    Custom: String,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawCategory {
    Named(String),
    Custom(CustomCategoryTable),
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawMethodMatcher {
    Any(String),
    Tagged {
        kind: String,
        methods: Option<Vec<String>>,
    },
}

fn parse_manifest_file(path: &Path) -> Result<SkillManifest, SkillManifestError> {
    let raw_text = std::fs::read_to_string(path).map_err(|source| SkillManifestError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let raw: RawSkillManifest = toml::from_str(&raw_text).map_err(|source| SkillManifestError::Parse {
        path: path.to_path_buf(),
        source,
    })?;

    let skill_id_raw = raw
        .skill_id
        .ok_or(SkillManifestError::MissingField {
            path: path.to_path_buf(),
            field: "skill_id",
        })?;
    let skill_id = SkillId::new(skill_id_raw);
    validate_filename(path, &skill_id)?;

    let wasm_path_raw = raw
        .wasm_path
        .ok_or(SkillManifestError::MissingField {
            path: path.to_path_buf(),
            field: "wasm_path",
        })?;
    let wasm_path = WasmBundlePath::new(wasm_path_raw);

    let applies_to_method = parse_method_matcher(
        raw.applies_to_method
            .ok_or(SkillManifestError::MissingField {
                path: path.to_path_buf(),
                field: "applies_to_method",
            })?,
        path,
    )?;

    let paths_raw = raw
        .applies_to_paths
        .ok_or(SkillManifestError::MissingField {
            path: path.to_path_buf(),
            field: "applies_to_paths",
        })?;
    if paths_raw.is_empty() {
        return Err(SkillManifestError::EmptyPaths {
            path: path.to_path_buf(),
        });
    }
    let applies_to_paths = paths_raw
        .into_iter()
        .map(JsonPathExpr::new)
        .collect::<Vec<_>>();

    let category = parse_category(
        raw.category
            .ok_or(SkillManifestError::MissingField {
                path: path.to_path_buf(),
                field: "category",
            })?,
    )?;

    let version_raw = raw
        .version
        .ok_or(SkillManifestError::MissingField {
            path: path.to_path_buf(),
            field: "version",
        })?;
    let version = SkillManifestVersion::new(version_raw)?;

    Ok(SkillManifest {
        skill_id,
        wasm_path,
        applies_to_method,
        applies_to_paths,
        category,
        version,
    })
}

fn validate_filename(path: &Path, skill_id: &SkillId) -> Result<(), SkillManifestError> {
    let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
        return Ok(());
    };
    let expected_stem = format!("{}.skill", skill_id.as_str());
    if stem != expected_stem {
        return Err(SkillManifestError::FilenameMismatch {
            path: path.to_path_buf(),
            expected: skill_id.clone(),
            found: SkillId::new(stem.trim_end_matches(".skill")),
        });
    }
    Ok(())
}

fn parse_method_matcher(
    raw: RawMethodMatcher,
    path: &Path,
) -> Result<SkillMethodMatcher, SkillManifestError> {
    match raw {
        RawMethodMatcher::Any(value) if value == "Any" => Ok(SkillMethodMatcher::Any),
        RawMethodMatcher::Tagged { kind, methods } => match kind.as_str() {
            "Any" => Ok(SkillMethodMatcher::Any),
            "OneOf" => {
                let methods_raw = methods.unwrap_or_default();
                let mut methods = Vec::with_capacity(methods_raw.len());
                for method in methods_raw {
                    methods.push(parse_method(path, method)?);
                }
                Ok(SkillMethodMatcher::OneOf(methods))
            }
            other => Err(SkillManifestError::InvalidMethod {
                path: path.to_path_buf(),
                method: other.to_string(),
            }),
        },
        RawMethodMatcher::Any(other) => Err(SkillManifestError::InvalidMethod {
            path: path.to_path_buf(),
            method: other,
        }),
    }
}

fn parse_method(path: &Path, method: String) -> Result<A2aMethod, SkillManifestError> {
    A2aMethod::from_str(&method).map_err(|_| SkillManifestError::InvalidMethod { path: path.to_path_buf(), method })
}

fn parse_category(raw: RawCategory) -> Result<SkillCategory, SkillManifestError> {
    match raw {
        RawCategory::Named(name) => SkillCategory::parse(&name),
        RawCategory::Custom(table) => Ok(SkillCategory::Custom(table.Custom)),
    }
}

fn is_semver_shaped(version: &str) -> bool {
    let core = version.split(['+', '-']).next().unwrap_or(version);
    let mut parts = core.split('.');
    let major = parts
        .next()
        .is_some_and(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()));
    let minor = parts
        .next()
        .is_some_and(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()));
    let patch = parts
        .next()
        .is_some_and(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()));
    major && minor && patch && parts.next().is_none()
}

#[cfg(test)]
#[path = "skill_manifest/tests.rs"]
mod tests;
