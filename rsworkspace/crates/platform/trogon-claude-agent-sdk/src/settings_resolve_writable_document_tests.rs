use std::{fs, io, os::unix::fs::symlink, path::Path};

use super::{DocumentProbe, Linkage, folder_owner, linkage};
use crate::{adapters::NativeHost, settings_resolve_state::ResolverState, settings_resolve_writable::stat_owner};

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

fn probe<'a>(root: &'a Path, components: &[&str], last_is_link: Option<bool>) -> DocumentProbe<'a> {
    DocumentProbe {
        root,
        components: components.iter().map(|component| (*component).to_owned()).collect(),
        last_is_link,
        opened: true,
    }
}

#[test]
fn a_document_under_a_root_that_cannot_be_examined_has_no_link() -> io::Result<()> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().canonicalize()?;
    assert_eq!(
        linkage_of(&root.join("absent"), "managed-settings.json"),
        Linkage::Direct
    );
    assert_eq!(linkage_of(&root, "missing.json"), Linkage::Direct);
    Ok(())
}

#[test]
fn a_root_that_is_a_link_links_every_document_under_it() -> io::Result<()> {
    let dir = tempfile::tempdir()?;
    let real = dir.path().canonicalize()?.join("real");
    fs::create_dir(&real)?;
    fs::write(real.join("managed-settings.json"), "{}")?;
    let linked = dir.path().canonicalize()?.join("linked");
    symlink(&real, &linked)?;
    assert_eq!(linkage_of(&linked, "managed-settings.json"), Linkage::Linked);
    assert_eq!(
        folder_owner(&NativeHost, &probe(&linked, &["managed-settings.json"], None)),
        None
    );
    Ok(())
}

#[test]
fn what_the_folder_listing_said_about_the_document_stands() -> io::Result<()> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().canonicalize()?;
    fs::write(root.join("real.json"), "{}")?;
    symlink("real.json", root.join("linked.json"))?;
    let state = ResolverState::new();
    assert_eq!(
        linkage(&NativeHost, &state, &probe(&root, &["linked.json"], Some(false))),
        Linkage::Direct
    );
    assert_eq!(
        linkage(&NativeHost, &state, &probe(&root, &["linked.json"], Some(true))),
        Linkage::Linked
    );
    Ok(())
}

#[test]
fn the_folder_owner_is_known_only_without_a_link_on_the_way() -> io::Result<()> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().canonicalize()?;
    let folder = root.join("managed.d");
    fs::create_dir(&folder)?;
    fs::write(folder.join("policy.json"), "{}")?;
    symlink(&folder, root.join("linked.d"))?;
    assert_eq!(
        folder_owner(&NativeHost, &probe(&root, &["managed.d", "policy.json"], None)),
        stat_owner(&NativeHost, &folder)
    );
    assert!(stat_owner(&NativeHost, &folder).is_some());
    assert_eq!(
        folder_owner(&NativeHost, &probe(&root, &["linked.d", "policy.json"], None)),
        None
    );
    assert_eq!(
        folder_owner(&NativeHost, &probe(&root, &["absent.d", "policy.json"], None)),
        None
    );
    Ok(())
}
