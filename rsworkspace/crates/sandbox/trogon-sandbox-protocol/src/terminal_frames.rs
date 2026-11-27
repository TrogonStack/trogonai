use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use trogon_sandbox_contract::{Base64Data, Base64DataError, TerminalInput, TerminalOutput};

/// Builds the `data` frame that publishes `bytes` as output number `seq`.
pub fn terminal_data_frame(seq: u64, bytes: &[u8]) -> Result<TerminalOutput, Base64DataError> {
    Ok(TerminalOutput::Data {
        seq,
        data: Base64Data::new(STANDARD.encode(bytes))?,
    })
}

/// The output number of a frame. A `resync` frame reports the number it resumes from.
pub fn terminal_output_seq(frame: &TerminalOutput) -> u64 {
    match frame {
        TerminalOutput::Data { seq, .. } | TerminalOutput::Exit { seq, .. } => *seq,
        TerminalOutput::Resync { from_seq } => *from_seq,
    }
}

/// Decodes the bytes of a `data` frame, or `None` for any other frame.
pub fn terminal_output_bytes(frame: &TerminalOutput) -> Option<Vec<u8>> {
    match frame {
        TerminalOutput::Data { data, .. } => STANDARD.decode(data.as_str()).ok(),
        TerminalOutput::Exit { .. } | TerminalOutput::Resync { .. } => None,
    }
}

/// Decodes the bytes of one terminal input write.
pub fn terminal_input_bytes(input: &TerminalInput) -> Result<Vec<u8>, base64::DecodeError> {
    STANDARD.decode(input.as_str())
}
