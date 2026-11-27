//! The committed `ateapi` bindings must be exactly what `proto/ateapi.proto` generates.
//!
//! After changing the schema, rerun this test with `TROGON_SANDBOX_ATEAPI_WRITE=1` to rewrite
//! `src/proto/substrate.ateapi.v1.rs`, then review the diff.

use std::path::{Path, PathBuf};

use trogon_std::env::{ReadEnv, SystemEnv};

const GENERATED: &str = "substrate.ateapi.v1.rs";
const WRITE_ENV: &str = "TROGON_SANDBOX_ATEAPI_WRITE";
/// The one service the worker calls. The schema's other services get no client, since nothing here would use it.
const CLIENT_SERVICE: &str = "Control";

#[derive(Debug, thiserror::Error)]
enum GeneratedProtoError {
    #[error("compiling the schema: {0}")]
    Compile(#[from] protox::Error),
    #[error("reading or writing the bindings: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0} is stale; rerun this test with {WRITE_ENV}=1 to regenerate it")]
    Stale(PathBuf),
}

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn committed_path() -> PathBuf {
    crate_dir().join("src/proto").join(GENERATED)
}

/// Generates the bindings the way the worker uses them: every message, and a client for `Control` only, since the
/// worker never serves `ateapi` and calls no other service.
fn generate(out_dir: &Path) -> Result<String, GeneratedProtoError> {
    let mut descriptors = protox::compile(["ateapi.proto"], [crate_dir().join("proto")])?;
    for file in &mut descriptors.file {
        file.service.retain(|service| service.name() == CLIENT_SERVICE);
    }
    tonic_prost_build::configure()
        .build_server(false)
        .build_client(true)
        .out_dir(out_dir)
        .compile_fds(descriptors)?;
    Ok(std::fs::read_to_string(out_dir.join(GENERATED))?)
}

#[test]
fn committed_bindings_match_the_schema() -> Result<(), GeneratedProtoError> {
    let out_dir = tempfile::tempdir()?;
    let expected = generate(out_dir.path())?;
    let path = committed_path();
    if SystemEnv.var_os(WRITE_ENV).is_some() {
        std::fs::write(&path, expected)?;
        return Ok(());
    }
    if std::fs::read_to_string(&path)? == expected {
        Ok(())
    } else {
        Err(GeneratedProtoError::Stale(path))
    }
}
