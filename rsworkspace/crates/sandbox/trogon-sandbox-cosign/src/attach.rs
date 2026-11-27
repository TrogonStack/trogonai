//! Cosign signing and attachment as an OCI referrer.
//!
//! AWS KMS (`awskms:///` URIs) is signed by the `cosign` CLI. Local PEM keys use the in-process
//! [`sigstore`] signer for tests and dev.

use std::collections::BTreeMap;
use std::process::Stdio;

use oci_client::client::{Config, ImageLayer};
use oci_client::manifest::{IMAGE_MANIFEST_MEDIA_TYPE, OciImageManifest};
use secrecy::{ExposeSecret, SecretString};
use sigstore::cosign::constraint::{AnnotationMarker, PrivateKeySigner};
use sigstore::cosign::{self, Constraint, CosignCapabilities, SignatureLayer};
use sigstore::crypto::SigningScheme;
use sigstore::registry::{Auth, ClientConfig, ClientProtocol, OciReference};
use tokio::process::Command;
use trogon_sandbox_contract::OciArtifactRef;
use trogon_sandbox_oci::{Annotations, Layer, OciClient};
use zeroize::Zeroizing;

use crate::constants::{COSIGN_SIMPLE_SIGNING_MEDIA_TYPE, SIGSTORE_SIGNATURE_ANNOTATION};
use crate::failure::{registry_failed, sign_failed};
use crate::{CosignError, CosignSigner};

/// Sign `subject` and attach the Cosign signature manifest as an OCI referrer.
pub async fn attach_cosign_signature(
    oci: &OciClient,
    subject: &OciArtifactRef,
    signer: &CosignSigner,
    insecure_registries: &[String],
) -> Result<(), CosignError> {
    attach_cosign_signature_with_annotations(oci, subject, signer, insecure_registries, &BTreeMap::new()).await
}

/// Like [`attach_cosign_signature`], and also binds `annotations` into the signed payload
/// (the `optional` map of the simple-signing document, as `cosign sign -a key=value` does),
/// so a verifier can trust them as much as the signature, for example the environment
/// digest of a snapshot.
pub async fn attach_cosign_signature_with_annotations(
    oci: &OciClient,
    subject: &OciArtifactRef,
    signer: &CosignSigner,
    insecure_registries: &[String],
    annotations: &BTreeMap<String, String>,
) -> Result<(), CosignError> {
    match signer {
        CosignSigner::AwsKms { uri, program } => {
            sign_with_cosign_cli(program, subject, uri, insecure_registries, annotations).await
        }
        CosignSigner::LocalKey {
            private_key_path,
            password,
        } => {
            let key = std::fs::read(private_key_path).map_err(sign_failed("read private key"))?;
            sign_in_process(oci, subject, &key, password.as_ref(), insecure_registries, annotations).await
        }
        CosignSigner::InMemoryKey {
            private_key_pem,
            password,
        } => {
            sign_in_process(
                oci,
                subject,
                private_key_pem.expose_secret(),
                password.as_ref(),
                insecure_registries,
                annotations,
            )
            .await
        }
    }
}

async fn sign_with_cosign_cli(
    program: &std::path::Path,
    subject: &OciArtifactRef,
    kms_uri: &str,
    insecure_registries: &[String],
    annotations: &BTreeMap<String, String>,
) -> Result<(), CosignError> {
    let mut cmd = Command::new(program);
    cmd.arg("sign")
        .arg("--key")
        .arg(kms_uri)
        .arg("--yes")
        .arg("--tlog-upload=false")
        .arg("--upload=true");

    for (key, value) in annotations {
        cmd.arg("-a").arg(format!("{key}={value}"));
    }

    if insecure_registries.contains(&subject.registry) {
        cmd.arg("--allow-insecure-registry");
    }

    cmd.arg(subject.allowlist_string());
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let output = cmd.output().await.map_err(sign_failed("spawn cosign"))?;

    if !output.status.success() {
        return Err(CosignError::SignFailed(format!(
            "cosign sign failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    Ok(())
}

async fn sign_in_process(
    oci: &OciClient,
    subject: &OciArtifactRef,
    private_key_pem: &[u8],
    password: Option<&SecretString>,
    insecure_registries: &[String],
    annotations: &BTreeMap<String, String>,
) -> Result<(), CosignError> {
    let image_ref: OciReference = subject
        .allowlist_string()
        .parse()
        .map_err(sign_failed("invalid image reference"))?;

    let mut oci_client_config = ClientConfig::default();
    if !insecure_registries.is_empty() {
        oci_client_config.protocol = ClientProtocol::HttpsExcept(insecure_registries.to_vec());
    }

    let mut cosign_client = cosign::ClientBuilder::default()
        .with_oci_client_config(oci_client_config)
        .build()
        .map_err(sign_failed("build cosign client"))?;

    let auth = Auth::Anonymous;
    let (cosign_sig_image, source_digest) = cosign_client
        .triangulate(&image_ref, &auth)
        .await
        .map_err(sign_failed("triangulate"))?;

    let mut signature_layer =
        SignatureLayer::new_unsigned(&image_ref, &source_digest).map_err(sign_failed("create signature layer"))?;

    if !annotations.is_empty() {
        AnnotationMarker::new(annotations.clone().into_iter().collect())
            .add_constraint(&mut signature_layer)
            .map_err(sign_failed("annotate signature"))?;
    }

    let key = Zeroizing::new(private_key_pem.to_vec());
    let password = Zeroizing::new(password.map_or_else(Vec::new, |secret| secret.expose_secret().as_bytes().to_vec()));
    let signing_scheme = SigningScheme::ECDSA_P256_SHA256_ASN1;
    let signer =
        PrivateKeySigner::new_with_raw(key, password, &signing_scheme).map_err(sign_failed("private key signer"))?;

    signer
        .add_constraint(&mut signature_layer)
        .map_err(sign_failed("sign layer"))?;

    let simple_signing_bytes =
        serde_json::to_vec(&signature_layer.simple_signing).map_err(sign_failed("serialize simple signing"))?;

    let mut layer_annotations = Annotations::new();
    if let Some(signature) = &signature_layer.signature {
        layer_annotations.insert(SIGSTORE_SIGNATURE_ANNOTATION.into(), signature.clone());
    }

    let layer = Layer {
        data: simple_signing_bytes,
        media_type: COSIGN_SIMPLE_SIGNING_MEDIA_TYPE.to_string(),
        annotations: layer_annotations,
    };

    let mut manifest = OciImageManifest::build(
        &[ImageLayer::new(
            bytes::Bytes::from(layer.data.clone()),
            layer.media_type.clone(),
            Some(layer.annotations.clone()),
        )],
        &Config::oci_v1(b"{}".as_ref(), None),
        None,
    );
    manifest.media_type = Some(IMAGE_MANIFEST_MEDIA_TYPE.to_string());

    let signature_size = i64::try_from(layer.data.len()).map_err(sign_failed("signature size"))?;
    let signature_digest = oci
        .push_raw_manifest(subject, &manifest, &[layer])
        .await
        .map_err(registry_failed("push cosign manifest"))?;

    oci.attach_referrer_with_media_type(
        subject,
        &signature_digest,
        signature_size,
        COSIGN_SIMPLE_SIGNING_MEDIA_TYPE,
    )
    .await
    .map_err(registry_failed("attach cosign referrer"))?;

    // Also push via the cosign client so verification finds the `.sig` tag.
    cosign_client
        .push_signature(None, &auth, &cosign_sig_image, vec![signature_layer])
        .await
        .map_err(sign_failed("push cosign signature tag"))?;

    Ok(())
}
