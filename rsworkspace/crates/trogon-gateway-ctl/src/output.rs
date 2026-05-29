use std::io::{self, Write};

use serde::Serialize;

pub fn emit_json<T: Serialize>(value: &T, pretty: bool) -> io::Result<()> {
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    if pretty {
        serde_json::to_writer_pretty(&mut handle, value)?;
    } else {
        serde_json::to_writer(&mut handle, value)?;
    }
    handle.write_all(b"\n")?;
    Ok(())
}

pub fn emit_toml<T: Serialize>(value: &T) -> io::Result<()> {
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    let rendered = toml::to_string_pretty(value).map_err(io::Error::other)?;
    handle.write_all(rendered.as_bytes())?;
    handle.write_all(b"\n")?;
    Ok(())
}
