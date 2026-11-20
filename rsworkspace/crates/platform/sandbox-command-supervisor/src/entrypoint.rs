use crate::protocol::{Event, ExecutionId, VERSION, encode_event};
use std::io::Write;

pub(crate) fn run(family_limit: usize) -> i32 {
    let arguments = match std::env::args_os()
        .skip(1)
        .map(|argument| argument.into_string())
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(arguments) => arguments,
        Err(_) => return 64,
    };
    if arguments.as_slice() == ["--probe"] {
        #[cfg(target_os = "linux")]
        let supported = crate::linux::probe().is_ok();
        #[cfg(not(target_os = "linux"))]
        let supported = false;
        let delivered = encode_event(&Event::Probe {
            version: VERSION,
            supported: supported.into(),
        })
        .and_then(|frame| write_probe_frame(&frame).map_err(crate::ProtocolError::Io));
        if delivered.is_err() {
            return 74;
        }
        return if supported { 0 } else { 78 };
    }
    let execution = match arguments.as_slice() {
        [flag, value] if flag == "--execution-id" => ExecutionId::new(value.clone()).ok(),
        _ => None,
    };
    let Some(execution) = execution else {
        return 64;
    };
    #[cfg(target_os = "linux")]
    return crate::linux::run(execution, family_limit);
    #[cfg(not(target_os = "linux"))]
    {
        let frame = Event::Error {
            execution_id: execution,
            code: crate::protocol::ErrorCode::UnsupportedPlatform,
            uncertain: false.into(),
        };
        if let Ok(frame) = encode_event(&frame) {
            let _ = std::io::stdout().write_all(&frame);
        }
        let _ = family_limit;
        78
    }
}

#[cfg(unix)]
fn write_probe_frame(frame: &[u8]) -> std::io::Result<()> {
    let descriptor = unsafe { std::os::fd::BorrowedFd::borrow_raw(1) }.try_clone_to_owned()?;
    std::fs::File::from(descriptor).write_all(frame)
}

#[cfg(not(unix))]
fn write_probe_frame(frame: &[u8]) -> std::io::Result<()> {
    let mut output = std::io::stdout().lock();
    output.write_all(frame)?;
    output.flush()
}
