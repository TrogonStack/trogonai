use std::fs;
use std::path::{Path, PathBuf};

use clap::Parser;
use ed25519_dalek::{Signer, SigningKey};

use a2a_redaction::signed_bundle::{
    Ed25519Signature, Sha256Digest, SignedBundleManifest, sign_bundle_digest,
};
use a2a_redaction::SkillId;

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

fn main() -> Result<(), String> {
    let args = Args::parse();
    let signing_key = parse_signing_key(&args.key)?;
    let skills = discover_skills(&args.skill_dir)?;
    if skills.is_empty() {
        return Err(format!("no *.wasm bundles found in {}", args.skill_dir.display()));
    }

    for skill in skills {
        sign_skill_bundle(&args.skill_dir, &skill, &signing_key)?;
        eprintln!("signed {}", skill.as_str());
    }

    Ok(())
}

fn parse_signing_key(raw: &str) -> Result<SigningKey, String> {
    let trimmed = raw.trim();
    if trimmed.starts_with("0x") || trimmed.starts_with("0X") {
        return Err("signing key must not use 0x prefix".into());
    }
    let decoded = hex::decode(trimmed).map_err(|err| format!("invalid signing key hex: {err}"))?;
    if decoded.len() != 32 {
        return Err(format!("signing key must be 32 bytes, got {}", decoded.len()));
    }
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&decoded);
    Ok(SigningKey::from_bytes(&seed))
}

fn discover_skills(dir: &Path) -> Result<Vec<SkillId>, String> {
    let mut skills = Vec::new();
    for entry in fs::read_dir(dir).map_err(|err| format!("read {}: {err}", dir.display()))? {
        let entry = entry.map_err(|err| format!("read dir entry: {err}"))?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("wasm") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        skills.push(SkillId::new(stem));
    }
    skills.sort();
    skills.dedup();
    Ok(skills)
}

fn sign_skill_bundle(dir: &Path, skill: &SkillId, signing_key: &SigningKey) -> Result<(), String> {
    let wasm_path = dir.join(format!("{}.wasm", skill.as_str()));
    let manifest_path = dir.join(format!("{}.manifest.json", skill.as_str()));
    let wasm_bytes = fs::read(&wasm_path).map_err(|err| format!("read {}: {err}", wasm_path.display()))?;
    let manifest_bytes =
        fs::read(&manifest_path).map_err(|err| format!("read {}: {err}", manifest_path.display()))?;

    let manifest_digest = Sha256Digest::hash(&manifest_bytes);
    let wasm_digest = Sha256Digest::hash(&wasm_bytes);
    let message = sign_bundle_digest(manifest_digest, wasm_digest);
    let signature = Ed25519Signature::from_bytes(signing_key.sign(&message).to_bytes());
    let envelope = SignedBundleManifest::new(skill, manifest_digest, wasm_digest, signature);
    let sig_path = dir.join(format!("{}.sig", skill.as_str()));
    let sig_json = serde_json::to_vec_pretty(&envelope).map_err(|err| err.to_string())?;
    fs::write(&sig_path, sig_json).map_err(|err| format!("write {}: {err}", sig_path.display()))
}
