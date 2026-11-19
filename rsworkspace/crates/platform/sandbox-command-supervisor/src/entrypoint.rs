use crate::protocol::{Event, ExecutionId, VERSION, encode_event};
use std::io::Write;

pub(crate) fn run(family_limit: usize) -> i32 {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if arguments.as_slice() == ["--probe"] {
        #[cfg(target_os = "linux")]
        let supported = crate::linux::probe().is_ok();
        #[cfg(not(target_os = "linux"))]
        let supported = false;
        if let Ok(frame) = encode_event(&Event::Probe {
            version: VERSION,
            supported: supported.into(),
        }) {
            let _ = std::io::stdout().write_all(&frame);
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
