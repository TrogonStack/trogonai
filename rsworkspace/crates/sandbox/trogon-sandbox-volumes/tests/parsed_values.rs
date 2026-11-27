#[cfg(unix)]
use std::ffi::OsStr;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use trogon_sandbox_volumes::{
    KmsKeyId, KmsKeyIdError, ManifestKey, ManifestKeyError, StoreResourceName, StoreResourceNameError, VolumeEndpoint,
    VolumeEndpointError, VolumePrefix, VolumePrefixError, VolumeRegion, VolumeRegionError,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn manifest_key_accepts_nested_relative_keys() -> TestResult {
    let key = ManifestKey::new("src/main.rs")?;
    assert_eq!(key.as_str(), "src/main.rs");
    assert_eq!(key.to_string(), "src/main.rs");
    assert_eq!("src/main.rs".parse::<ManifestKey>()?, key);
    assert_eq!(
        key.to_path(Path::new("base")),
        Path::new("base").join("src").join("main.rs")
    );
    Ok(())
}

#[test]
fn manifest_key_rejects_unsafe_keys() {
    assert_eq!(ManifestKey::new(""), Err(ManifestKeyError::Empty));
    assert_eq!(ManifestKey::new("/a"), Err(ManifestKeyError::EmptySegment));
    assert_eq!(ManifestKey::new("a//b"), Err(ManifestKeyError::EmptySegment));
    assert_eq!(ManifestKey::new("a\\b"), Err(ManifestKeyError::Backslash));
    assert_eq!(
        ManifestKey::new("a/../b"),
        Err(ManifestKeyError::RelativeSegment("..".to_owned()))
    );
    assert_eq!(
        ManifestKey::new("./a"),
        Err(ManifestKeyError::RelativeSegment(".".to_owned()))
    );
    assert!("../a".parse::<ManifestKey>().is_err());
}

#[test]
fn manifest_key_from_relative_path_requires_plain_names() -> TestResult {
    assert_eq!(
        ManifestKey::from_relative_path(Path::new("a").join("b.txt").as_path())?.as_str(),
        "a/b.txt"
    );
    assert_eq!(
        ManifestKey::from_relative_path(Path::new("../a")),
        Err(ManifestKeyError::NotRelative)
    );
    assert_eq!(
        ManifestKey::from_relative_path(Path::new("/a")),
        Err(ManifestKeyError::NotRelative)
    );
    assert_eq!(
        ManifestKey::from_relative_path(Path::new("")),
        Err(ManifestKeyError::Empty)
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn manifest_key_from_relative_path_rejects_non_utf8() {
    let path = Path::new(OsStr::from_bytes(&[0x66, 0xff]));
    assert_eq!(ManifestKey::from_relative_path(path), Err(ManifestKeyError::NonUnicode));
}

#[test]
fn prefix_is_normalized() -> TestResult {
    assert_eq!(VolumePrefix::new("  /tenants/acme/ ")?.as_str(), "tenants/acme");
    assert_eq!("a/b".parse::<VolumePrefix>()?.to_string(), "a/b");
    assert!(VolumePrefix::new("")?.is_root());
    assert!(VolumePrefix::new(" / ")?.is_root());
    assert_eq!(VolumePrefix::root(), VolumePrefix::new("")?);
    assert!(!VolumePrefix::new("a")?.is_root());
    Ok(())
}

#[test]
fn prefix_rejects_traversal_and_empty_segments() {
    assert!(VolumePrefix::new("a/../b").is_err());
    assert!(VolumePrefix::new("a//b").is_err());
    let err = VolumePrefix::new("..").err();
    assert!(matches!(err, Some(VolumePrefixError(_))));
}

#[test]
fn token_types_trim_and_validate() -> TestResult {
    assert_eq!(VolumeRegion::new(" us-east-1 ")?.as_str(), "us-east-1");
    assert_eq!("eu".parse::<VolumeRegion>()?.to_string(), "eu");
    assert_eq!(VolumeRegion::new("  "), Err(VolumeRegionError::Empty));
    assert_eq!(VolumeRegion::new("us east"), Err(VolumeRegionError::InvalidCharacter));

    assert_eq!(StoreResourceName::new("acct")?.as_str(), "acct");
    assert_eq!("acct".parse::<StoreResourceName>()?.to_string(), "acct");
    assert_eq!(StoreResourceName::new(""), Err(StoreResourceNameError::Empty));
    assert_eq!(
        StoreResourceName::new("a\tb"),
        Err(StoreResourceNameError::InvalidCharacter)
    );

    assert_eq!(KmsKeyId::new("key-1")?.as_str(), "key-1");
    assert_eq!("key-1".parse::<KmsKeyId>()?.to_string(), "key-1");
    assert_eq!(KmsKeyId::new(""), Err(KmsKeyIdError::Empty));
    assert_eq!(KmsKeyId::new("a b"), Err(KmsKeyIdError::InvalidCharacter));
    Ok(())
}

#[test]
fn endpoint_requires_http_or_https_with_host() -> TestResult {
    let plain = VolumeEndpoint::new(" http://127.0.0.1:9000 ")?;
    assert!(plain.is_plain_http());
    assert_eq!(plain.as_str(), "http://127.0.0.1:9000");
    assert_eq!(plain.to_string(), "http://127.0.0.1:9000");

    let secure: VolumeEndpoint = "https://s3.example.com".parse()?;
    assert!(!secure.is_plain_http());

    assert!(matches!(
        VolumeEndpoint::new("not a url"),
        Err(VolumeEndpointError::Malformed(_))
    ));
    assert_eq!(
        VolumeEndpoint::new("ftp://host"),
        Err(VolumeEndpointError::UnsupportedScheme("ftp".to_owned()))
    );
    assert!(VolumeEndpoint::new("http://").is_err());
    Ok(())
}
