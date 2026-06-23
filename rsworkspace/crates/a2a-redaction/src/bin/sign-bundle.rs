use std::fs;
use std::path::{Path, PathBuf};

use clap::Parser;
use ed25519_dalek::{Signer, SigningKey};

use a2a_redaction::signed_bundle::{
    Ed25519Signature, SIGNED_BUNDLE_VERSION, Sha256Digest, SignedBundleManifest, sign_bundle_digest,
};
use a2a_redaction::{SkillId, SkillIdError};

#[derive(Debug, Parser)]
#[command(name = "a2a-sign-bundle", about = "Sign Tier-3 WASM policy bundles")]
struct Args {
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
    #[error("serialize signature envelope for skill {skill}: {source}")]
    SerializeSignature {
        skill: String,
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

fn main() -> Result<(), CliError> {
    let args = Args::parse();
    let signing_key = parse_signing_key(&args.key)?;
    let skills = discover_skills(&args.skill_dir)?;
    if skills.is_empty() {
        return Err(CliError::NoSkillBundles(args.skill_dir));
    }

    for skill in skills {
        sign_skill_bundle(&args.skill_dir, &skill, &signing_key)?;
        eprintln!("signed {}", skill.as_str());
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
    let mut skills = Vec::new();
    for entry in fs::read_dir(dir).map_err(|source| CliError::ReadDir {
        path: dir.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| CliError::ReadDirEntry {
            path: dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
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
    let sig_json = serde_json::to_vec_pretty(&envelope).map_err(|source| CliError::SerializeSignature {
        skill: skill.to_string(),
        source,
    })?;
    fs::write(&sig_path, sig_json).map_err(|source| CliError::WriteFile { path: sig_path, source })
}
