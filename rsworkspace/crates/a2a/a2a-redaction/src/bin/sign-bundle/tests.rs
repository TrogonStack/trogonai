use std::error::Error;
use std::io::ErrorKind;
#[cfg(unix)]
use std::{ffi::OsString, os::unix::ffi::OsStringExt};

use a2a_redaction::signed_bundle::{Ed25519PublicKey, verify_signed_bundle};

use super::*;

#[test]
fn input_requires_key_and_skill_directory() {
    assert!(matches!(
        SignBundlesInput::try_parse_from(["a2a-sign-bundle", "--key", "00"]),
        Err(error) if error.kind() == clap::error::ErrorKind::MissingRequiredArgument
    ));
    assert!(matches!(
        SignBundlesInput::try_parse_from(["a2a-sign-bundle", "--skill-dir", "skills"]),
        Err(error) if error.kind() == clap::error::ErrorKind::MissingRequiredArgument
    ));
}

#[test]
fn signing_key_accepts_whitespace_and_mixed_case_hex() -> Result<(), Box<dyn Error>> {
    let key = parse_signing_key(&format!(" \t{}\n", "aB".repeat(32)))?;
    assert_eq!(key.to_bytes(), [0xab; 32]);
    Ok(())
}

#[test]
fn signing_key_rejects_hex_prefixes() {
    for prefix in ["0x", "0X"] {
        let raw = format!(" {prefix}{} ", "00".repeat(32));
        assert!(matches!(parse_signing_key(&raw), Err(CliError::KeyHasHexPrefix)));
    }
}

#[test]
fn signing_key_preserves_hex_decode_errors() {
    assert!(matches!(
        parse_signing_key("zz"),
        Err(CliError::KeyHexDecode(hex::FromHexError::InvalidHexCharacter {
            c: 'z',
            index: 0
        }))
    ));
    assert!(matches!(
        parse_signing_key("0"),
        Err(CliError::KeyHexDecode(hex::FromHexError::OddLength))
    ));
}

#[test]
fn signing_key_reports_decoded_length() {
    for length in [0, 31, 33] {
        assert!(matches!(
            parse_signing_key(&"00".repeat(length)),
            Err(CliError::KeyWrongLength(actual)) if actual == length
        ));
    }
}

#[test]
fn discovery_sorts_skills_and_skips_other_extensions() -> Result<(), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    for filename in [
        "zebra.wasm",
        "alpha.wasm",
        "alpha.sig",
        "alpha.manifest.json",
        "UPPER.WASM",
        "README",
    ] {
        fs::write(dir.path().join(filename), [])?;
    }

    let skills = discover_skills(dir.path())?;
    assert_eq!(
        skills.iter().map(SkillId::as_str).collect::<Vec<_>>(),
        ["alpha", "zebra"]
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn discovery_skips_non_utf8_skill_paths() -> Result<(), Box<dyn Error>> {
    let dir = Path::new("skills");
    let entries = [
        Ok(dir.join(OsString::from_vec(b"\xff.wasm".to_vec()))),
        Ok(dir.join("valid.wasm")),
    ];

    assert_eq!(discover_skill_paths(dir, entries)?, [SkillId::new("valid")?]);
    Ok(())
}

#[test]
fn discovery_preserves_iterator_failure_without_returning_partial_skills() {
    let dir = Path::new("skills");
    let entries = [
        Ok(dir.join("alpha.wasm")),
        Err(std::io::Error::from(ErrorKind::PermissionDenied)),
    ];
    assert!(matches!(
        discover_skill_paths(dir, entries),
        Err(CliError::ReadDirEntry { path, source }) if path == dir && source.kind() == ErrorKind::PermissionDenied
    ));
}

struct BrokenSignatureWriter;

impl std::io::Write for BrokenSignatureWriter {
    fn write(&mut self, _bytes: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::from(ErrorKind::BrokenPipe))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn signature_write_reports_output_path_and_underlying_io_failure() -> Result<(), Box<dyn Error>> {
    let skill = SkillId::new("demo")?;
    let envelope = SignedBundleManifest::new(
        &skill,
        Sha256Digest::hash(b"manifest"),
        Sha256Digest::hash(b"wasm"),
        Ed25519Signature::from_bytes([0; 64]),
    );
    let path = Path::new("skills/demo.sig");
    assert!(matches!(
        write_signature(BrokenSignatureWriter, path, &envelope),
        Err(CliError::WriteSignature { path: actual, source }) if actual == path && source.io_error_kind() == Some(ErrorKind::BrokenPipe)
    ));
    Ok(())
}

#[test]
fn discovery_preserves_invalid_skill_path_and_cause() -> Result<(), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let invalid_path = dir.path().join("invalid..skill.wasm");
    fs::write(&invalid_path, [])?;

    assert!(matches!(
        discover_skills(dir.path()),
        Err(CliError::InvalidSkillId { path, source: SkillIdError::PathTraversal }) if path == invalid_path
    ));
    Ok(())
}

#[test]
fn run_rejects_invalid_key_before_discovery() -> Result<(), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let input = SignBundlesInput {
        key: "0x00".to_owned(),
        skill_dir: dir.path().join("missing"),
    };

    assert!(matches!(run(input, |_| {}), Err(CliError::KeyHasHexPrefix)));
    Ok(())
}

#[test]
fn run_rejects_invalid_discovered_skill_before_signing() -> Result<(), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    fs::write(dir.path().join("alpha.wasm"), b"\0asm")?;
    fs::write(dir.path().join("alpha.manifest.json"), b"{}")?;
    let invalid_path = dir.path().join("invalid..skill.wasm");
    fs::write(&invalid_path, b"\0asm")?;
    let input = SignBundlesInput {
        key: "07".repeat(32),
        skill_dir: dir.path().to_path_buf(),
    };
    let mut reported = Vec::new();

    assert!(matches!(
        run(input, |skill| reported.push(skill.clone())),
        Err(CliError::InvalidSkillId { path, source: SkillIdError::PathTraversal }) if path == invalid_path
    ));
    assert!(reported.is_empty());
    assert!(!dir.path().join("alpha.sig").exists());
    Ok(())
}

#[test]
fn run_reports_missing_directory_with_path_and_cause() -> Result<(), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let missing = dir.path().join("missing");
    let input = SignBundlesInput {
        key: "07".repeat(32),
        skill_dir: missing.clone(),
    };

    assert!(matches!(
        run(input, |_| {}),
        Err(CliError::ReadDir { path, source }) if path == missing && source.kind() == ErrorKind::NotFound
    ));
    Ok(())
}

#[test]
fn run_rejects_directories_without_wasm_bundles() -> Result<(), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    fs::write(dir.path().join("demo.manifest.json"), b"{}")?;
    let input = SignBundlesInput {
        key: "07".repeat(32),
        skill_dir: dir.path().to_path_buf(),
    };

    assert!(matches!(run(input, |_| {}), Err(CliError::NoSkillBundles(path)) if path == dir.path()));
    Ok(())
}

#[test]
fn run_signs_every_bundle_with_verifiable_digests() -> Result<(), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let bundles = [
        (
            "alpha",
            br#"{"skill_id":"alpha"}"#.as_slice(),
            b"\0asm-alpha".as_slice(),
        ),
        (
            "zebra",
            br#"{"skill_id":"zebra"}"#.as_slice(),
            b"\0asm-zebra".as_slice(),
        ),
    ];
    for (skill, manifest, wasm) in bundles {
        fs::write(dir.path().join(format!("{skill}.manifest.json")), manifest)?;
        fs::write(dir.path().join(format!("{skill}.wasm")), wasm)?;
    }
    let input = SignBundlesInput::try_parse_from([
        "a2a-sign-bundle".as_ref(),
        "--key".as_ref(),
        "07".repeat(32).as_ref(),
        "--skill-dir".as_ref(),
        dir.path().as_os_str(),
    ])?;

    let mut reported = Vec::new();
    run(input, |skill| {
        assert!(dir.path().join(format!("{skill}.sig")).is_file());
        reported.push(skill.clone());
    })?;
    assert_eq!(reported, [SkillId::new("alpha")?, SkillId::new("zebra")?]);

    let public_key = Ed25519PublicKey::from_bytes(SigningKey::from_bytes(&[7; 32]).verifying_key().to_bytes());
    for (skill, manifest, wasm) in bundles {
        let skill_id = SkillId::new(skill)?;
        let signature = fs::read(dir.path().join(format!("{skill}.sig")))?;
        let envelope = SignedBundleManifest::parse_json(&signature, &skill_id)?;
        verify_signed_bundle(&public_key, manifest, wasm, &envelope)?;
        assert_eq!(fs::read(dir.path().join(format!("{skill}.manifest.json")))?, manifest);
        assert_eq!(fs::read(dir.path().join(format!("{skill}.wasm")))?, wasm);
    }
    Ok(())
}

#[test]
fn signing_reports_missing_wasm_with_path_and_cause() -> Result<(), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let skill = SkillId::new("demo")?;
    let key = SigningKey::from_bytes(&[7; 32]);

    assert!(matches!(
        sign_skill_bundle(dir.path(), &skill, &key),
        Err(CliError::ReadFile { path, source })
            if path == dir.path().join("demo.wasm") && source.kind() == ErrorKind::NotFound
    ));
    assert!(!dir.path().join("demo.sig").exists());
    Ok(())
}

#[test]
fn run_stops_at_missing_manifest_after_signing_earlier_bundles() -> Result<(), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    for skill in ["alpha", "missing", "zebra"] {
        fs::write(dir.path().join(format!("{skill}.wasm")), b"\0asm")?;
    }
    fs::write(dir.path().join("alpha.manifest.json"), b"{}")?;
    fs::write(dir.path().join("zebra.manifest.json"), b"{}")?;
    let input = SignBundlesInput {
        key: "07".repeat(32),
        skill_dir: dir.path().to_path_buf(),
    };

    let mut reported = Vec::new();
    let result = run(input, |skill| {
        assert!(dir.path().join(format!("{skill}.sig")).is_file());
        reported.push(skill.clone());
    });
    assert!(matches!(
        result,
        Err(CliError::ReadFile { path, source })
            if path == dir.path().join("missing.manifest.json") && source.kind() == ErrorKind::NotFound
    ));
    assert_eq!(reported, [SkillId::new("alpha")?]);
    assert!(dir.path().join("alpha.sig").is_file());
    assert!(!dir.path().join("missing.sig").exists());
    assert!(!dir.path().join("zebra.sig").exists());
    Ok(())
}

#[test]
fn signing_reports_output_write_failure_with_path_and_cause() -> Result<(), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let skill = SkillId::new("demo")?;
    let key = SigningKey::from_bytes(&[7; 32]);
    let output_path = dir.path().join("demo.sig");
    fs::write(dir.path().join("demo.wasm"), b"\0asm")?;
    fs::write(dir.path().join("demo.manifest.json"), b"{}")?;
    fs::create_dir(&output_path)?;

    assert!(matches!(
        sign_skill_bundle(dir.path(), &skill, &key),
        Err(CliError::WriteFile { path, source }) if path == output_path && source.kind() == ErrorKind::IsADirectory
    ));
    Ok(())
}
