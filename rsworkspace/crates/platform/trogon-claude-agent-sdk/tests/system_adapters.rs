use std::{
    error::Error as StdError,
    ffi::{OsStr, OsString},
    fs::{DirBuilder, OpenOptions},
    io::{Read, Write},
    path::PathBuf,
    time::UNIX_EPOCH,
};
use trogon_claude_agent_sdk::adapters::{
    CanonicalizePath, CreateDirectory, CurrentDirectory, EnvironmentVariable, NativeFileSystem, OpenFile, ReadMetadata,
    RemoveDirectoryAll, RemoveFile, SystemClock, SystemEnvironment, TemporaryDirectory, WallClock,
};
#[cfg(unix)]
use trogon_claude_agent_sdk::adapters::{
    HomeDirectory, NativeHost, OpenDirectoryNoFollow, ReadDeviceId, ReadFileHead, ReadLink,
};

type TestResult = Result<(), Box<dyn StdError>>;

fn unique_directory() -> Result<PathBuf, Box<dyn StdError>> {
    let nanos = SystemClock.now().duration_since(UNIX_EPOCH)?.as_nanos();
    Ok(SystemEnvironment
        .temp_dir()
        .join(format!("claude-sdk-system-adapters-{}-{nanos}", std::process::id())))
}

#[test]
fn system_environment_reads_variables_and_directories() -> TestResult {
    let missing = OsString::from("CLAUDE_SDK_ADAPTER_TEST_VARIABLE_THAT_IS_NEVER_SET");
    assert_eq!(SystemEnvironment.var_os(&missing), None);
    assert!(SystemEnvironment.var_os(OsStr::new("PATH")).is_some());
    assert!(SystemEnvironment.current_dir()?.is_absolute());
    assert!(!SystemEnvironment.temp_dir().as_os_str().is_empty());
    Ok(())
}

#[test]
fn system_clock_reports_a_time_after_the_unix_epoch() -> TestResult {
    assert!(SystemClock.now().duration_since(UNIX_EPOCH)?.as_secs() > 0);
    Ok(())
}

#[test]
fn native_file_system_creates_opens_inspects_and_removes_entries() -> TestResult {
    let root = unique_directory()?;
    let nested = root.join("nested");
    let file = nested.join("entry.txt");

    NativeFileSystem.create_directory(DirBuilder::new().recursive(true), &nested)?;
    assert!(NativeFileSystem.metadata(&nested)?.is_dir());

    let mut writer = NativeFileSystem.open(OpenOptions::new().write(true).create_new(true), &file)?;
    writer.write_all(b"adapter bytes")?;
    drop(writer);
    assert_eq!(NativeFileSystem.metadata(&file)?.len(), 13);

    let mut contents = String::new();
    NativeFileSystem
        .open(OpenOptions::new().read(true), &file)?
        .read_to_string(&mut contents)?;
    assert_eq!(contents, "adapter bytes");
    assert_eq!(NativeFileSystem.canonicalize(&file)?.file_name(), file.file_name());

    NativeFileSystem.remove_file(&file)?;
    assert!(NativeFileSystem.metadata(&file).is_err());
    NativeFileSystem.remove_dir_all(&root)?;
    assert!(NativeFileSystem.canonicalize(&root).is_err());
    Ok(())
}

#[cfg(unix)]
#[test]
fn native_host_probes_folders_links_heads_and_devices_without_following_links() -> TestResult {
    let root = unique_directory()?;
    DirBuilder::new().recursive(true).create(root.join("folder"))?;
    std::fs::write(root.join("file"), b"abcdef")?;
    std::os::unix::fs::symlink("folder", root.join("link"))?;
    let host = NativeHost;
    host.open_directory_no_follow(&root.join("folder"))?;
    assert!(host.open_directory_no_follow(&root.join("link")).is_err());
    assert!(host.open_directory_no_follow(&root.join("file")).is_err());
    assert_eq!(host.read_link(&root.join("link"))?, PathBuf::from("folder"));
    assert!(host.read_link(&root.join("file")).is_err());
    assert_eq!(host.read_file_head(&root.join("file"), 4)?, b"abcd");
    assert_eq!(host.read_file_head(&root.join("file"), 16)?, b"abcdef");
    assert!(host.read_file_head(&root.join("link"), 4).is_err());
    assert_eq!(
        host.device_id(&root.join("folder"))?,
        host.device_id(&root.join("file"))?
    );
    assert!(host.device_id(&root.join("missing")).is_err());
    host.home_directory()?;
    std::fs::remove_dir_all(&root)?;
    Ok(())
}
