use std::{fs, io, os::unix::fs::symlink, path::Path};

use super::{DocumentProbe, Linkage, linkage};
use crate::{adapters::NativeHost, settings_resolve_state::ResolverState};

fn linkage_of(root: &Path, name: &str) -> Linkage {
    let probe = DocumentProbe {
        root,
        components: vec![name.to_owned()],
        last_is_link: None,
        opened: true,
    };
    linkage(&NativeHost, &ResolverState::new(), &probe)
}

#[test]
fn a_document_reached_through_a_link_is_located_where_the_link_leads() -> io::Result<()> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().canonicalize()?;
    fs::write(root.join("real.json"), "{}")?;
    symlink("real.json", root.join("linked.json"))?;
    symlink("nothing.json", root.join("dangling.json"))?;
    symlink("\u{fffd}.json", root.join("undecodable.json"))?;
    assert_eq!(linkage_of(&root, "real.json"), Linkage::Direct);
    assert_eq!(linkage_of(&root, "linked.json"), Linkage::Linked);
    assert_eq!(linkage_of(&root, "dangling.json"), Linkage::Direct);
    assert_eq!(linkage_of(&root, "undecodable.json"), Linkage::Unverified);
    Ok(())
}
