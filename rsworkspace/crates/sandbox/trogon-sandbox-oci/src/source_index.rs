use std::collections::BTreeMap;

use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SourceIndex<'a> {
    pub(crate) media_type: &'a str,
    pub(crate) manifests: [SourceManifest<'a>; 1],
    pub(crate) schema_version: u8,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SourceManifest<'a> {
    pub(crate) annotations: BTreeMap<&'a str, &'a str>,
    pub(crate) digest: &'a str,
    pub(crate) media_type: &'a str,
    pub(crate) size: usize,
}
