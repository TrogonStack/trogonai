use serde_json::Value;
use trogon_sandbox_protocol::CappedOutput;

/// What the NDJSON records of one command add up to.
pub(crate) struct Collected {
    pub(crate) stdout: CappedOutput,
    pub(crate) stderr: CappedOutput,
    pub(crate) exit: Option<i32>,
    pub(crate) command_id: Option<String>,
    pub(crate) error: Option<String>,
    pub(crate) truncated: bool,
}

impl Collected {
    pub(crate) fn with_limit(limit: usize) -> Self {
        Self {
            stdout: CappedOutput::new(limit),
            stderr: CappedOutput::new(limit),
            exit: None,
            command_id: None,
            error: None,
            truncated: false,
        }
    }

    fn push(&mut self, stderr: bool, data: &[u8]) {
        let buf = if stderr { &mut self.stderr } else { &mut self.stdout };
        buf.push(data);
        self.truncated |= buf.truncation().is_truncated();
    }

    /// Folds one NDJSON record in. A line that is not JSON carries nothing and is ignored.
    pub(crate) fn line(&mut self, line: &[u8]) {
        let Ok(v) = serde_json::from_slice::<Value>(line) else {
            return;
        };
        if let Some(stream) = v.get("stream").and_then(Value::as_str) {
            let data = v.get("data");
            match stream {
                "stdout" | "stderr" => {
                    if let Some(text) = data.and_then(Value::as_str) {
                        self.push(stream == "stderr", text.as_bytes());
                    }
                }
                "error" => {
                    let message = data
                        .and_then(|d| d.get("message"))
                        .or(data)
                        .and_then(Value::as_str)
                        .unwrap_or("command stream error");
                    self.error = Some(message.chars().take(300).collect());
                }
                _ => {}
            }
        } else if let Some(command) = v.get("command") {
            if let Some(id) = command.get("id").and_then(Value::as_str) {
                self.command_id = Some(id.to_string());
            }
            if let Some(code) = command.get("exitCode").and_then(Value::as_i64) {
                self.exit = Some(i32::try_from(code).unwrap_or(-1));
            }
        }
    }
}

#[cfg(test)]
mod tests;
