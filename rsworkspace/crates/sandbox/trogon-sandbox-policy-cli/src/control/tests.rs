use std::io::Write;

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[cfg(unix)]
fn descriptor_of(end: impl std::os::fd::IntoRawFd) -> Result<ControlFd, Box<dyn std::error::Error>> {
    ControlFd::parse(&std::os::fd::IntoRawFd::into_raw_fd(end).to_string()).map_err(|error| error.to_string().into())
}

#[cfg(windows)]
fn descriptor_of(end: impl std::os::windows::io::IntoRawHandle) -> Result<ControlFd, Box<dyn std::error::Error>> {
    unsafe extern "C" {
        fn _open_osfhandle(handle: isize, flags: i32) -> i32;
    }
    let fd = unsafe { _open_osfhandle(end.into_raw_handle() as isize, 0) };
    if fd < 0 {
        return Err("the pipe end has no descriptor".into());
    }
    ControlFd::parse(&fd.to_string()).map_err(|error| error.to_string().into())
}

#[test]
fn lines_written_to_the_pipe_arrive_and_the_end_closes_the_channel() -> TestResult {
    let (reader, mut writer) = std::io::pipe()?;
    let channel = ControlChannel::open(descriptor_of(reader)?)?;
    writer.write_all(b"{\"a\":1}\n{\"b\":2}\n")?;
    drop(writer);
    let mut events = channel.listen();
    assert!(matches!(events.blocking_recv(), Some(ControlEvent::Line(line)) if line == "{\"a\":1}\n"));
    assert!(matches!(events.blocking_recv(), Some(ControlEvent::Line(line)) if line == "{\"b\":2}\n"));
    assert!(matches!(events.blocking_recv(), Some(ControlEvent::Closed)));
    Ok(())
}

#[test]
fn the_write_end_is_refused() -> TestResult {
    let (_reader, writer) = std::io::pipe()?;
    let refusal = ControlChannel::open(descriptor_of(writer)?)
        .err()
        .ok_or("the write end was accepted")?;
    assert!(refusal.contains("open for writing only"), "{refusal}");
    Ok(())
}

#[test]
fn a_descriptor_that_is_not_open_is_refused() -> TestResult {
    let refusal = ControlChannel::open(ControlFd::parse("900").map_err(|error| error.to_string())?)
        .err()
        .ok_or("a closed descriptor was accepted")?;
    assert!(
        refusal.contains("bad file descriptor") || refusal.contains("Bad file descriptor"),
        "{refusal}"
    );
    Ok(())
}

#[cfg(windows)]
#[test]
fn the_original_handle_is_no_longer_inheritable() -> TestResult {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetHandleInformation(handle: isize, flags: *mut u32) -> i32;
    }
    let (reader, _writer) = std::io::pipe()?;
    let handle = std::os::windows::io::AsRawHandle::as_raw_handle(&reader) as isize;
    ControlChannel::open(descriptor_of(reader)?)?;
    let mut flags = 0u32;
    assert_ne!(unsafe { GetHandleInformation(handle, &mut flags) }, 0);
    assert_eq!(flags & 1, 0);
    Ok(())
}
