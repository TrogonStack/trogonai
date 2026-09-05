#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

use {
    a2a_redaction::signed_bundle::{
        Ed25519Signature, SIGNED_BUNDLE_VERSION, Sha256Digest, SignedBundleManifest, sign_bundle_digest,
    },
    a2a_redaction::{SkillId, SkillIdError},
    clap::Parser,
    ed25519_dalek::{Signer, SigningKey},
    std::fs,
    std::path::{Path, PathBuf},
};

#[derive(Debug, Parser)]
#[command(name = "a2a-sign-bundle", about = "Sign Tier-3 WASM policy bundles")]
struct SignBundlesInput {
    /// Hex-encoded 32-byte ed25519 signing key seed (64 hex chars, no 0x prefix)
    #[arg(long)]
    key: String,

    /// Directory containing `{skill}.wasm` and `{skill}.manifest.json` pairs
    #[arg(long)]
    skill_dir: PathBuf,
}

#[derive(Debug, thiserror::Error)]
enum CliError {
    #[error("signing key must not use 0x prefix")]
    KeyHasHexPrefix,
    #[error("invalid signing key hex: {0}")]
    KeyHexDecode(#[source] hex::FromHexError),
    #[error("signing key must be 32 bytes, got {0}")]
    KeyWrongLength(usize),
    #[error("read dir {path}: {source}", path = path.display())]
    ReadDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("read dir entry under {path}: {source}", path = path.display())]
    ReadDirEntry {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid skill id derived from {path}: {source}", path = path.display())]
    InvalidSkillId {
        path: PathBuf,
        #[source]
        source: SkillIdError,
    },
    #[error("no *.wasm bundles found in {}", .0.display())]
    NoSkillBundles(PathBuf),
    #[error("read {path}: {source}", path = path.display())]
    ReadFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("write signature {path}: {source}", path = path.display())]
    WriteSignature {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("write {path}: {source}", path = path.display())]
    WriteFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[cfg_attr(
    dylint_lib = "trogon_lints",
    allow(
        debug_remnants,
        reason = "the per-bundle progress is this CLI's own output to the operator running it"
    )
)]
#[cfg_attr(coverage_nightly, coverage(off))]
fn main() -> Result<(), CliError> {
    run(SignBundlesInput::parse(), |skill| {
        eprintln!("signed {}", skill.as_str())
    })
}

fn run(input: SignBundlesInput, report_signed: impl FnMut(&SkillId)) -> Result<(), CliError> {
    let signing_key = parse_signing_key(&input.key)?;
    let skills = discover_skills(&input.skill_dir)?;
    if skills.is_empty() {
        return Err(CliError::NoSkillBundles(input.skill_dir));
    }

    sign_bundles(&input.skill_dir, &skills, &signing_key, report_signed)
}

fn sign_bundles(
    dir: &Path,
    skills: &[SkillId],
    signing_key: &SigningKey,
    mut report_signed: impl FnMut(&SkillId),
) -> Result<(), CliError> {
    for skill in skills {
        sign_skill_bundle(dir, skill, signing_key)?;
        report_signed(skill);
    }

    Ok(())
}

fn parse_signing_key(raw: &str) -> Result<SigningKey, CliError> {
    let trimmed = raw.trim();
    if trimmed.starts_with("0x") || trimmed.starts_with("0X") {
        return Err(CliError::KeyHasHexPrefix);
    }
    let decoded = hex::decode(trimmed).map_err(CliError::KeyHexDecode)?;
    if decoded.len() != 32 {
        return Err(CliError::KeyWrongLength(decoded.len()));
    }
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&decoded);
    Ok(SigningKey::from_bytes(&seed))
}

fn discover_skills(dir: &Path) -> Result<Vec<SkillId>, CliError> {
    let entries = fs::read_dir(dir).map_err(|source| CliError::ReadDir {
        path: dir.to_path_buf(),
        source,
    })?;
    discover_skill_paths(dir, entries.map(|entry| entry.map(|entry| entry.path())))
}

fn discover_skill_paths(
    dir: &Path,
    entries: impl IntoIterator<Item = std::io::Result<PathBuf>>,
) -> Result<Vec<SkillId>, CliError> {
    let mut skills = Vec::new();
    for entry in entries {
        let path = entry.map_err(|source| CliError::ReadDirEntry {
            path: dir.to_path_buf(),
            source,
        })?;
        if path.extension().and_then(|ext| ext.to_str()) != Some("wasm") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let skill = SkillId::new(stem).map_err(|source| CliError::InvalidSkillId {
            path: path.clone(),
            source,
        })?;
        skills.push(skill);
    }
    skills.sort();
    skills.dedup();
    Ok(skills)
}

fn sign_skill_bundle(dir: &Path, skill: &SkillId, signing_key: &SigningKey) -> Result<(), CliError> {
    let wasm_path = dir.join(format!("{}.wasm", skill.as_str()));
    let manifest_path = dir.join(format!("{}.manifest.json", skill.as_str()));
    let wasm_bytes = fs::read(&wasm_path).map_err(|source| CliError::ReadFile {
        path: wasm_path.clone(),
        source,
    })?;
    let manifest_bytes = fs::read(&manifest_path).map_err(|source| CliError::ReadFile {
        path: manifest_path.clone(),
        source,
    })?;

    let manifest_digest = Sha256Digest::hash(&manifest_bytes);
    let wasm_digest = Sha256Digest::hash(&wasm_bytes);
    let message = sign_bundle_digest(SIGNED_BUNDLE_VERSION, skill, manifest_digest, wasm_digest);
    let signature = Ed25519Signature::from_bytes(signing_key.sign(&message).to_bytes());
    let envelope = SignedBundleManifest::new(skill, manifest_digest, wasm_digest, signature);
    let sig_path = dir.join(format!("{}.sig", skill.as_str()));
    let file = fs::File::create(&sig_path).map_err(|source| CliError::WriteFile {
        path: sig_path.clone(),
        source,
    })?;
    write_signature(file, &sig_path, &envelope)
}

fn write_signature(writer: impl std::io::Write, path: &Path, envelope: &SignedBundleManifest) -> Result<(), CliError> {
    serde_json::to_writer_pretty(writer, envelope).map_err(|source| CliError::WriteSignature {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
#[path = "sign-bundle/tests.rs"]
mod tests;
