use trogon_mcp_gateway::bundle::{
    build_tar, hash_member, BundleArchive, BundleManifest, Capabilities, ComponentEntry,
    ProgramEntry, Signing, HOST_TARGET_WIT, MANIFEST_FILENAME,
};

use crate::cel::{
    AUDIT_ID, AUDIT_PATH, CATALOG_FILTER_ID, CATALOG_FILTER_PATH, DEFAULT_AUDIT,
    DEFAULT_CATALOG_FILTER, DEFAULT_RESOURCE_TUPLE, RESOURCE_TUPLE_ID, RESOURCE_TUPLE_PATH,
};
use crate::components::{
    SCHEMA_LEARNER_ID, SCHEMA_LEARNER_PATH, SCHEMA_LEARNER_STUB,
};
use crate::sign::{sign_manifest, signature_archive_path};
use crate::{McpPackError, McpPackSpec};

pub struct BundleMember {
    pub path: &'static str,
    pub bytes: &'static [u8],
}

pub fn default_members(include_schema_learner: bool) -> Vec<BundleMember> {
    let mut members = vec![
        BundleMember {
            path: CATALOG_FILTER_PATH,
            bytes: DEFAULT_CATALOG_FILTER.as_bytes(),
        },
        BundleMember {
            path: RESOURCE_TUPLE_PATH,
            bytes: DEFAULT_RESOURCE_TUPLE.as_bytes(),
        },
        BundleMember {
            path: AUDIT_PATH,
            bytes: DEFAULT_AUDIT.as_bytes(),
        },
    ];
    if include_schema_learner {
        members.push(BundleMember {
            path: SCHEMA_LEARNER_PATH,
            bytes: SCHEMA_LEARNER_STUB,
        });
    }
    members
}

pub fn assemble_manifest(spec: &McpPackSpec, members: &[BundleMember]) -> Result<BundleManifest, McpPackError> {
    let programs = vec![
        ProgramEntry {
            id: CATALOG_FILTER_ID.into(),
            path: CATALOG_FILTER_PATH.into(),
            sha256: hash_member(members[0].bytes),
            class: "tools_list_filter".into(),
            effect: "allow".into(),
            priority: 50,
        },
        ProgramEntry {
            id: RESOURCE_TUPLE_ID.into(),
            path: RESOURCE_TUPLE_PATH.into(),
            sha256: hash_member(members[1].bytes),
            class: "ingress_gate".into(),
            effect: "allow".into(),
            priority: 100,
        },
        ProgramEntry {
            id: AUDIT_ID.into(),
            path: AUDIT_PATH.into(),
            sha256: hash_member(members[2].bytes),
            class: "audit_enrich".into(),
            effect: "allow".into(),
            priority: 10,
        },
    ];

    let mut components = Vec::new();
    if spec.include_schema_learner {
        let wasm = members
            .iter()
            .find(|member| member.path == SCHEMA_LEARNER_PATH)
            .ok_or_else(|| {
                McpPackError::Build("schema-learner member missing".into())
            })?;
        components.push(ComponentEntry {
            id: SCHEMA_LEARNER_ID.into(),
            path: SCHEMA_LEARNER_PATH.into(),
            sha256: hash_member(wasm.bytes),
            mode: Some("advisory".into()),
        });
    }

    Ok(BundleManifest {
        name: spec.name.clone(),
        version: spec.version.clone(),
        target_wit: HOST_TARGET_WIT.into(),
        min_gateway_version: spec.min_gateway_version.clone(),
        cel_version: Some(spec.cel_version.clone()),
        author: spec.author.clone(),
        created_at: spec.created_at.clone(),
        description: spec.description.clone(),
        capabilities: Capabilities {
            imports: vec![
                "host.spicedb-check".into(),
                "host.audit-emit".into(),
            ],
        },
        signing: Signing {
            nkey_pub: spec.signer.public_key(),
        },
        programs,
        components,
        schemas: vec![],
    })
}

pub fn assemble_archive(spec: &McpPackSpec) -> Result<Vec<u8>, McpPackError> {
    let members = default_members(spec.include_schema_learner);
    let manifest = assemble_manifest(spec, &members)?;
    let manifest_bytes = manifest.to_toml().map_err(McpPackError::Manifest)?;
    let signature = sign_manifest(&spec.signer, manifest_bytes.as_bytes())?;

    let mut archive = BundleArchive::default();
    archive.insert(MANIFEST_FILENAME, manifest_bytes.into_bytes());
    for member in &members {
        archive.insert(member.path, member.bytes.to_vec());
    }
    archive.insert(signature_archive_path(), signature);
    Ok(build_tar(&archive))
}
